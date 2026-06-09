use std::{
    fmt::{self, Display},
    str::FromStr,
};

use itertools::Itertools;
use miette::Diagnostic;
use pixi_consts::consts;
use pixi_core::DependencyType;
use pixi_manifest::{FeatureName, PixiPlatform, PixiPlatformName, SpecType, WorkspaceManifest};
use pixi_pypi_spec::PypiPackageName;
use rattler_conda_types::PackageName;

/// Diagnostic for the "dependency not found" path of `pixi remove`. Carries
/// computed help text that points the user at the right dependency table,
/// feature, or a similar-looking package name.
#[derive(Debug)]
pub(super) struct DependencyRemovalError {
    name: String,
    dependency_type: DependencyType,
    feature: FeatureName,
    suggestions: Vec<String>,
}

impl DependencyRemovalError {
    pub(super) fn new(
        name: String,
        manifest: &WorkspaceManifest,
        dependency_type: DependencyType,
        feature: &FeatureName,
        platforms: &[PixiPlatformName],
    ) -> Self {
        let suggestions = collect_suggestions(manifest, &name, dependency_type, feature, platforms);
        Self {
            name,
            dependency_type,
            feature: feature.clone(),
            suggestions,
        }
    }
}

impl Display for DependencyRemovalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "dependency `{}` was not found in {}",
            self.name,
            Slot::from(self.dependency_type).table_name()
        )?;
        if !self.feature.is_default() {
            write!(f, " of feature `{}`", self.feature)?;
        }
        Ok(())
    }
}

impl std::error::Error for DependencyRemovalError {}

impl Diagnostic for DependencyRemovalError {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        if self.suggestions.is_empty() {
            None
        } else {
            Some(Box::new(self.suggestions.join("\n")))
        }
    }
}

/// Local key that identifies which dependency table a package lives in.
/// Mirrors [`DependencyType`] but is `Eq` so we can deduplicate locations.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Slot {
    Conda(SpecType),
    Pypi,
}

impl From<DependencyType> for Slot {
    fn from(value: DependencyType) -> Self {
        match value {
            DependencyType::CondaDependency(s) => Slot::Conda(s),
            DependencyType::PypiDependency => Slot::Pypi,
        }
    }
}

impl Slot {
    fn table_name(self) -> &'static str {
        match self {
            Slot::Pypi => "pypi-dependencies",
            Slot::Conda(SpecType::Host) => "host-dependencies",
            Slot::Conda(SpecType::Build) => "build-dependencies",
            Slot::Conda(SpecType::Run) => "dependencies",
            Slot::Conda(SpecType::RunConstraints) => "run-constraints",
        }
    }

    fn cli_flag(self) -> Option<&'static str> {
        match self {
            Slot::Pypi => Some("--pypi"),
            Slot::Conda(SpecType::Host) => Some("--host"),
            Slot::Conda(SpecType::Build) => Some("--build"),
            // `Run` is the default; `RunConstraints` cannot be removed via the CLI.
            Slot::Conda(_) => None,
        }
    }
}

/// One dependency entry discovered while walking the workspace, used by the
/// suggestion collector to match against the missing name.
struct DepEntry<'a> {
    feature: &'a FeatureName,
    slot: Slot,
    name: String,
}

fn collect_suggestions(
    manifest: &WorkspaceManifest,
    name: &str,
    dependency_type: DependencyType,
    feature: &FeatureName,
    platforms: &[PixiPlatformName],
) -> Vec<String> {
    let current_slot = Slot::from(dependency_type);
    let target_conda = PackageName::try_from(name).ok();
    let target_pypi = PypiPackageName::from_str(name).ok();

    let mut exact_locations: Vec<(FeatureName, Slot)> = Vec::new();
    let mut similar_in_target: Vec<String> = Vec::new();

    for entry in walk_dependencies(manifest, platforms) {
        if is_exact_match(&entry, target_conda.as_ref(), target_pypi.as_ref()) {
            let location = (entry.feature.clone(), entry.slot);
            if (entry.feature, entry.slot) != (feature, current_slot)
                && !exact_locations.contains(&location)
            {
                exact_locations.push(location);
            }
            continue;
        }

        let same_slot_and_feature = entry.slot == current_slot && entry.feature == feature;
        if same_slot_and_feature
            && is_similar(name, &entry.name)
            && !similar_in_target.contains(&entry.name)
        {
            similar_in_target.push(entry.name);
        }
    }

    let mut suggestions = Vec::new();
    for (feat_name, slot) in exact_locations {
        suggestions.push(format_exact_location(name, slot, &feat_name));
    }
    if !similar_in_target.is_empty() {
        let quoted = similar_in_target
            .iter()
            .map(|s| format!("`{s}`"))
            .join(", ");
        suggestions.push(format!("did you mean {quoted}?"));
    }
    suggestions
}

/// Flatten the (feature × platform × spec-type) iteration into a single
/// stream of dependency entries so the matching logic above can stay flat.
fn walk_dependencies<'a>(
    manifest: &'a WorkspaceManifest,
    platforms: &[PixiPlatformName],
) -> impl Iterator<Item = DepEntry<'a>> + 'a {
    let platform_opts = to_platform_options(manifest, platforms);
    manifest.features.iter().flat_map(move |(feat_name, feat)| {
        platform_opts
            .clone()
            .into_iter()
            .flat_map(move |platform| dependencies_for(feat_name, feat, platform))
    })
}

fn dependencies_for<'a>(
    feat_name: &'a FeatureName,
    feat: &'a pixi_manifest::Feature,
    platform: Option<&PixiPlatform>,
) -> Vec<DepEntry<'a>> {
    let mut entries = Vec::new();
    for spec_type in SpecType::all() {
        if let Some(deps) = feat.dependencies(spec_type, platform) {
            entries.extend(deps.iter().map(|(pkg, _)| DepEntry {
                feature: feat_name,
                slot: Slot::Conda(spec_type),
                name: pkg.as_normalized().to_string(),
            }));
        }
    }
    if let Some(deps) = feat.pypi_dependencies(platform) {
        entries.extend(deps.iter().map(|(pkg, _)| DepEntry {
            feature: feat_name,
            slot: Slot::Pypi,
            name: pkg.as_source().to_string(),
        }));
    }
    entries
}

fn is_exact_match(
    entry: &DepEntry<'_>,
    target_conda: Option<&PackageName>,
    target_pypi: Option<&PypiPackageName>,
) -> bool {
    match entry.slot {
        Slot::Conda(_) => target_conda
            .and_then(|t| {
                PackageName::try_from(entry.name.as_str())
                    .ok()
                    .map(|n| n == *t)
            })
            .unwrap_or(false),
        Slot::Pypi => target_pypi
            .and_then(|t| PypiPackageName::from_str(&entry.name).ok().map(|n| n == *t))
            .unwrap_or(false),
    }
}

fn format_exact_location(name: &str, slot: Slot, feature: &FeatureName) -> String {
    let feature_part = if feature.is_default() {
        "the default feature".to_string()
    } else {
        format!("feature `{}`", consts::FEATURE_STYLE.apply_to(feature))
    };
    let mut parts = vec!["pixi".to_string(), "remove".to_string()];
    if let Some(flag) = slot.cli_flag() {
        parts.push(flag.to_string());
    }
    if !feature.is_default() {
        parts.push("--feature".to_string());
        parts.push(feature.to_string());
    }
    parts.push(name.to_string());
    let invocation = parts.join(" ");
    format!(
        "`{name}` is a {} entry in {feature_part}; try `{invocation}`",
        slot.table_name()
    )
}

fn is_similar(a: &str, b: &str) -> bool {
    a != b && strsim::jaro(a, b) > 0.85
}

fn to_platform_options<'a>(
    manifest: &'a WorkspaceManifest,
    platforms: &[PixiPlatformName],
) -> Vec<Option<&'a PixiPlatform>> {
    if platforms.is_empty() {
        vec![None]
    } else {
        platforms
            .iter()
            .filter_map(|name| manifest.workspace.platform_by_name(name))
            .map(Some)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pixi_manifest::SpecType;
    use pixi_test_utils::format_diagnostic;

    use super::*;

    fn parse(toml: &str) -> WorkspaceManifest {
        WorkspaceManifest::from_toml_str_with_base_dir(toml, Path::new(""))
            .expect("failed to parse manifest")
    }

    fn render(
        manifest: &WorkspaceManifest,
        name: &str,
        dep_type: DependencyType,
        feature: &FeatureName,
        platforms: &[PixiPlatformName],
    ) -> String {
        let err =
            DependencyRemovalError::new(name.to_string(), manifest, dep_type, feature, platforms);
        format_diagnostic(&err)
    }

    const MIXED_MANIFEST: &str = r#"
[workspace]
name = "test"
channels = []
platforms = ["linux-64"]

[dependencies]
ruff = "*"

[pypi-dependencies]
polars = "*"

[host-dependencies]
openssl = "*"

[build-dependencies]
cmake = "*"

[feature.dev.dependencies]
numpy = "*"

[feature.dev.pypi-dependencies]
pandas = "*"
"#;

    #[test]
    fn missing_dep_lives_in_pypi() {
        // `pixi remove polars` (no flag) when polars is a pypi-dependency.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "polars",
                DependencyType::CondaDependency(SpecType::Run),
                &FeatureName::DEFAULT,
                &[],
            ),
            @r"
          × dependency `polars` was not found in dependencies
          help: `polars` is a pypi-dependencies entry in the default feature; try `pixi remove --pypi polars`
        "
        );
    }

    #[test]
    fn missing_dep_lives_in_conda() {
        // `pixi remove --pypi ruff` when ruff is a conda dependency.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "ruff",
                DependencyType::PypiDependency,
                &FeatureName::DEFAULT,
                &[],
            ),
            @r"
          × dependency `ruff` was not found in pypi-dependencies
          help: `ruff` is a dependencies entry in the default feature; try `pixi remove ruff`
        "
        );
    }

    #[test]
    fn missing_dep_lives_in_host_deps() {
        // `pixi remove openssl` when openssl is in host-dependencies.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "openssl",
                DependencyType::CondaDependency(SpecType::Run),
                &FeatureName::DEFAULT,
                &[],
            ),
            @r"
          × dependency `openssl` was not found in dependencies
          help: `openssl` is a host-dependencies entry in the default feature; try `pixi remove --host openssl`
        "
        );
    }

    #[test]
    fn missing_dep_lives_in_build_deps() {
        // `pixi remove cmake` when cmake is in build-dependencies.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "cmake",
                DependencyType::CondaDependency(SpecType::Run),
                &FeatureName::DEFAULT,
                &[],
            ),
            @r"
          × dependency `cmake` was not found in dependencies
          help: `cmake` is a build-dependencies entry in the default feature; try `pixi remove --build cmake`
        "
        );
    }

    #[test]
    fn missing_dep_lives_in_other_feature() {
        // `pixi remove numpy` from the default feature when numpy is only in
        // feature `dev`.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "numpy",
                DependencyType::CondaDependency(SpecType::Run),
                &FeatureName::DEFAULT,
                &[],
            ),
            @r"
          × dependency `numpy` was not found in dependencies
          help: `numpy` is a dependencies entry in feature `dev`; try `pixi remove --feature dev numpy`
        "
        );
    }

    #[test]
    fn missing_dep_typo_suggests_similar_name() {
        // `pixi remove --pypi polrs` when polars exists. Jaro similarity
        // catches the typo.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "polrs",
                DependencyType::PypiDependency,
                &FeatureName::DEFAULT,
                &[],
            ),
            @r"
          × dependency `polrs` was not found in pypi-dependencies
          help: did you mean `polars`?
        "
        );
    }

    #[test]
    fn missing_dep_truly_absent() {
        // `pixi remove fizzbuzz` with nothing matching. No help text.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "fizzbuzz",
                DependencyType::CondaDependency(SpecType::Run),
                &FeatureName::DEFAULT,
                &[],
            ),
            @"  × dependency `fizzbuzz` was not found in dependencies"
        );
    }

    #[test]
    fn missing_dep_wrong_dep_type_in_non_default_feature() {
        // `pixi remove --pypi numpy --feature dev`: numpy exists in feature
        // dev but as a conda dep, not a pypi dep.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "numpy",
                DependencyType::PypiDependency,
                &FeatureName::from("dev"),
                &[],
            ),
            @r"
          × dependency `numpy` was not found in pypi-dependencies of feature `dev`
          help: `numpy` is a dependencies entry in feature `dev`; try `pixi remove --feature dev numpy`
        "
        );
    }

    #[test]
    fn missing_dep_pypi_in_non_default_feature() {
        // `pixi remove pandas --feature dev`: pandas exists in feature dev
        // but as a pypi dep.
        let manifest = parse(MIXED_MANIFEST);
        insta::assert_snapshot!(
            render(
                &manifest,
                "pandas",
                DependencyType::CondaDependency(SpecType::Run),
                &FeatureName::from("dev"),
                &[],
            ),
            @r"
          × dependency `pandas` was not found in dependencies of feature `dev`
          help: `pandas` is a pypi-dependencies entry in feature `dev`; try `pixi remove --pypi --feature dev pandas`
        "
        );
    }
}
