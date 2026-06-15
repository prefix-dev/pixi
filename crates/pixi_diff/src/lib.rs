use std::{
    collections::HashSet,
    io::{Write, stderr},
};

use ahash::HashMap;
use indexmap::IndexMap;
use itertools::{Either, Itertools};
use pixi_consts::consts;
use pixi_manifest::{EnvironmentName, FeaturesExt, PixiPlatformName};
use rattler_lock::{CondaPackageData, LockFile, LockedPackage, PlatformName};
use serde::Serialize;
use serde_json::Value;
use tabwriter::TabWriter;

// Represents the differences between two sets of packages.
#[derive(Default, Clone)]
pub struct PackagesDiff {
    pub added: Vec<LockedPackage>,
    pub removed: Vec<LockedPackage>,
    pub changed: Vec<(LockedPackage, LockedPackage)>,
}

impl PackagesDiff {
    /// Returns true if the diff is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Contains the changes between two lock files.
pub struct LockFileDiff {
    pub environment: IndexMap<String, IndexMap<PlatformName, PackagesDiff>>,
}

impl LockFileDiff {
    /// Determine the difference between two lock files.
    pub fn from_lock_files(previous: &LockFile, current: &LockFile) -> Self {
        let mut result = Self {
            environment: IndexMap::new(),
        };

        for (environment_name, environment) in current.environments() {
            let previous = previous.environment(environment_name);

            let mut environment_diff = IndexMap::new();

            for (lock_platform, packages) in environment.packages_by_platform() {
                // Key by rattler's PlatformName so foreign/hand-edited names
                // (invalid pixi platform names) still appear instead of crashing.
                let platform = lock_platform.name().clone();
                // Determine the packages that were previously there.
                let (mut previous_conda_packages, mut previous_pypi_packages): (
                    HashMap<_, _>,
                    HashMap<_, _>,
                ) = previous
                    .as_ref()
                    .and_then(|e| {
                        let p = e.lock_file().platform(lock_platform.name())?;
                        e.packages(p)
                    })
                    .into_iter()
                    .flatten()
                    .partition_map(|p| match p {
                        LockedPackage::Conda(conda_package_data) => {
                            Either::Left((conda_package_data.name().clone(), conda_package_data))
                        }
                        LockedPackage::Pypi(pypi_package_data) => {
                            Either::Right((pypi_package_data.name().clone(), pypi_package_data))
                        }
                    });

                let mut diff = PackagesDiff::default();

                // Find new and changed packages
                for package in packages {
                    match package {
                        LockedPackage::Conda(data) => {
                            let name = data.name();
                            match previous_conda_packages.remove(name) {
                                Some(previous) if previous.location() != data.location() => {
                                    diff.changed.push((
                                        LockedPackage::Conda(previous.clone()),
                                        LockedPackage::Conda(data.clone()),
                                    ));
                                }
                                None => {
                                    diff.added.push(LockedPackage::Conda(data.clone()));
                                }
                                _ => {}
                            }
                        }
                        LockedPackage::Pypi(data) => {
                            let name = data.name();
                            match previous_pypi_packages.remove(name) {
                                Some(previous_data)
                                    if previous_data.location() != data.location() =>
                                {
                                    diff.changed.push((
                                        LockedPackage::Pypi(previous_data.clone()),
                                        LockedPackage::Pypi(data.clone()),
                                    ));
                                }
                                None => {
                                    diff.added.push(LockedPackage::Pypi(data.clone()));
                                }
                                _ => {}
                            }
                        }
                    }
                }

                // Determine packages that were removed
                for (_, p) in previous_conda_packages {
                    diff.removed.push(LockedPackage::Conda(p.clone()));
                }
                for (_, data) in previous_pypi_packages {
                    diff.removed.push(LockedPackage::Pypi(data.clone()));
                }

                environment_diff.insert(platform, diff);
            }

            // Find platforms that were completely removed
            for (lock_platform, packages) in previous
                .as_ref()
                .map(|e| e.packages_by_platform())
                .into_iter()
                .flatten()
                .filter(|(p, _)| !environment_diff.contains_key(p.name()))
                .collect_vec()
            {
                let platform = lock_platform.name().clone();
                let mut diff = PackagesDiff::default();
                for package in packages {
                    diff.removed.push(package.clone());
                }
                environment_diff.insert(platform, diff);
            }

            // Remove empty diffs
            environment_diff.retain(|_, diff| !diff.is_empty());

            result
                .environment
                .insert(environment_name.to_string(), environment_diff);
        }

        // Find environments that were completely removed
        for (environment_name, environment) in previous
            .environments()
            .filter(|(name, _)| !result.environment.contains_key(*name))
            .collect_vec()
        {
            let mut environment_diff = IndexMap::new();
            for (lock_platform, packages) in environment.packages_by_platform() {
                let mut diff = PackagesDiff::default();
                for package in packages {
                    diff.removed.push(package.clone());
                }
                let platform = lock_platform.name().clone();
                environment_diff.insert(platform, diff);
            }
            result
                .environment
                .insert(environment_name.to_string(), environment_diff);
        }

        // Remove empty environments
        result.environment.retain(|_, diff| !diff.is_empty());

        result
    }

    /// Returns true if the diff is empty.
    pub fn is_empty(&self) -> bool {
        self.environment.is_empty()
    }

    // Format the lock file diff.
    pub fn print(&self) -> std::io::Result<()> {
        let mut writer = TabWriter::new(stderr());
        for (idx, (environment_name, environment)) in self
            .environment
            .iter()
            .sorted_by(|(a, _), (b, _)| a.cmp(b))
            .enumerate()
        {
            // Find the changes that happened in all platforms.
            let changes_by_platform = environment
                .into_iter()
                .map(|(platform, packages)| {
                    let changes = Self::format_changes(packages)
                        .into_iter()
                        .collect::<HashSet<_>>();
                    (platform, changes)
                })
                .collect::<Vec<_>>();

            // Find the changes that happened in all platforms.
            let common_changes = changes_by_platform
                .iter()
                .fold(None, |acc, (_, changes)| match acc {
                    None => Some(changes.clone()),
                    Some(acc) => Some(acc.intersection(changes).cloned().collect()),
                })
                .unwrap_or_default();

            // Add a new line between environments
            if idx > 0 {
                writeln!(writer, "\t\t\t",)?;
            }

            writeln!(
                writer,
                "{}: {}\t\t\t",
                console::style("Environment").underlined(),
                consts::ENVIRONMENT_STYLE.apply_to(environment_name)
            )?;

            // Print the common changes.
            for (_, line) in common_changes.iter().sorted_by_key(|(name, _)| name) {
                writeln!(writer, "  {line}")?;
            }

            // Print the per-platform changes.
            for (platform, changes) in changes_by_platform {
                let mut changes = changes
                    .iter()
                    .filter(|change| !common_changes.contains(change))
                    .sorted_by_key(|(name, _)| name)
                    .peekable();
                if changes.peek().is_some() {
                    writeln!(
                        writer,
                        "{}: {}:{}\t\t\t",
                        console::style("Platform").underlined(),
                        consts::ENVIRONMENT_STYLE.apply_to(environment_name),
                        consts::PLATFORM_STYLE.apply_to(platform),
                    )?;
                    for (_, line) in changes {
                        writeln!(writer, "  {line}")?;
                    }
                }
            }
        }

        writer.flush()?;

        Ok(())
    }

    fn format_changes(packages: &PackagesDiff) -> Vec<(&str, String)> {
        enum Change<'i> {
            Added(&'i LockedPackage),
            Removed(&'i LockedPackage),
            Changed(&'i LockedPackage, &'i LockedPackage),
        }

        fn format_conda_identifier(p: &CondaPackageData) -> String {
            match p {
                CondaPackageData::Binary(b) => {
                    format!(
                        "{} {}",
                        b.package_record.version.as_str(),
                        b.package_record.build
                    )
                }
                CondaPackageData::Source(s) => {
                    format!("@ {}", s.location)
                }
            }
        }

        fn format_package_identifier(package: &LockedPackage) -> String {
            match package {
                LockedPackage::Conda(p) => format_conda_identifier(p),
                LockedPackage::Pypi(p) => p.version_string(),
            }
        }

        itertools::chain!(
            packages.added.iter().map(Change::Added),
            packages.removed.iter().map(Change::Removed),
            packages.changed.iter().map(|a| Change::Changed(&a.0, &a.1))
        )
        .sorted_by_key(|c| match c {
            Change::Added(p) => p.name(),
            Change::Removed(p) => p.name(),
            Change::Changed(p, _) => p.name(),
        })
        .map(|p| match p {
            Change::Added(p) => (
                p.name(),
                format!(
                    "{} {} {}\t{}\t\t",
                    console::style("+").green(),
                    match p {
                        LockedPackage::Conda(_) => consts::CondaEmoji.to_string(),
                        LockedPackage::Pypi(..) => consts::PypiEmoji.to_string(),
                    },
                    p.name(),
                    format_package_identifier(p)
                ),
            ),
            Change::Removed(p) => (
                p.name(),
                format!(
                    "{} {} {}\t{}\t\t",
                    console::style("-").red(),
                    match p {
                        LockedPackage::Conda(_) => consts::CondaEmoji.to_string(),
                        LockedPackage::Pypi(..) => consts::PypiEmoji.to_string(),
                    },
                    p.name(),
                    format_package_identifier(p)
                ),
            ),
            Change::Changed(previous, current) => {
                fn choose_style<'a>(a: &'a str, b: &'a str) -> console::StyledObject<&'a str> {
                    if a == b {
                        console::style(a).dim()
                    } else {
                        console::style(a)
                    }
                }

                let name = previous.name();
                let line = match (previous, current) {
                    (LockedPackage::Conda(previous), LockedPackage::Conda(current)) => {
                        match (previous, current) {
                            (CondaPackageData::Binary(prev), CondaPackageData::Binary(curr)) => {
                                let prev_ver = prev.package_record.version.as_str();
                                let curr_ver = curr.package_record.version.as_str();
                                format!(
                                    "{} {} {}\t{} {}\t->\t{} {}",
                                    console::style("~").yellow(),
                                    consts::CondaEmoji,
                                    name,
                                    choose_style(&prev_ver, &curr_ver),
                                    choose_style(
                                        prev.package_record.build.as_str(),
                                        curr.package_record.build.as_str()
                                    ),
                                    choose_style(&curr_ver, &prev_ver),
                                    choose_style(
                                        curr.package_record.build.as_str(),
                                        prev.package_record.build.as_str()
                                    ),
                                )
                            }
                            (CondaPackageData::Source(prev), CondaPackageData::Source(curr)) => {
                                let prev_loc = prev.location.to_string();
                                let curr_loc = curr.location.to_string();
                                format!(
                                    "{} {} {}\t@ {}\t->\t@ {}",
                                    console::style("~").yellow(),
                                    consts::CondaEmoji,
                                    name,
                                    choose_style(&prev_loc, &curr_loc),
                                    choose_style(&curr_loc, &prev_loc),
                                )
                            }
                            (CondaPackageData::Binary(prev), CondaPackageData::Source(curr)) => {
                                format!(
                                    "{} {} {}\t{} {}\t->\t@ {}",
                                    console::style("~").yellow(),
                                    consts::CondaEmoji,
                                    name,
                                    prev.package_record.version.as_str(),
                                    prev.package_record.build,
                                    curr.location,
                                )
                            }
                            (CondaPackageData::Source(prev), CondaPackageData::Binary(curr)) => {
                                format!(
                                    "{} {} {}\t@ {}\t->\t{} {}",
                                    console::style("~").yellow(),
                                    consts::CondaEmoji,
                                    name,
                                    prev.location,
                                    curr.package_record.version.as_str(),
                                    curr.package_record.build,
                                )
                            }
                        }
                    }
                    (LockedPackage::Pypi(previous), LockedPackage::Pypi(current)) => {
                        let prev_ver = previous.version_string();
                        let curr_ver = current.version_string();
                        format!(
                            "{} {} {}\t{}\t->\t{}",
                            console::style("~").yellow(),
                            consts::PypiEmoji,
                            name,
                            choose_style(&prev_ver, &curr_ver),
                            choose_style(&curr_ver, &prev_ver),
                        )
                    }
                    _ => unreachable!(),
                };

                (name, line)
            }
        })
        .collect()
    }
}

#[derive(Serialize, Clone)]
pub struct JsonPackageDiff {
    name: String,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
    #[serde(rename = "type")]
    ty: JsonPackageType,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    explicit: bool,
}

#[derive(Serialize, Copy, Clone)]
#[serde(rename_all = "kebab-case")]
pub enum JsonPackageType {
    Conda,
    Pypi,
}

#[derive(Serialize, Clone)]
pub struct LockFileJsonDiff {
    pub version: usize,
    pub environment: IndexMap<String, IndexMap<String, Vec<JsonPackageDiff>>>,
}

impl LockFileJsonDiff {
    pub fn new<'a, F: FeaturesExt<'a>>(
        environments: Option<std::collections::HashMap<EnvironmentName, F>>,
        value: LockFileDiff,
    ) -> Self {
        let mut environment = IndexMap::new();

        for (environment_name, environment_diff) in value.environment {
            let mut environment_diff_json = IndexMap::new();

            for (platform, packages_diff) in environment_diff {
                let env = environments
                    .as_ref()
                    .and_then(|p| p.get(environment_name.as_str()));
                let pixi_platform = env.and_then(|env| {
                    let name = PixiPlatformName::try_from(platform.as_str()).ok()?;
                    env.workspace_manifest().workspace.platform_by_name(&name)
                });
                let conda_dependencies = env
                    .map(|env| env.dependencies(pixi_manifest::SpecType::Run, pixi_platform))
                    .unwrap_or_default();

                let pypi_dependencies = env
                    .map(|env| env.pypi_dependencies(pixi_platform))
                    .unwrap_or_default();

                let add_diffs = packages_diff.added.into_iter().map(|new| match new {
                    LockedPackage::Conda(pkg) => JsonPackageDiff {
                        name: pkg.name().as_normalized().to_string(),
                        before: None,
                        after: Some(
                            serde_json::to_value(&pkg).expect("should be able to serialize"),
                        ),
                        ty: JsonPackageType::Conda,
                        explicit: conda_dependencies.contains_key(pkg.name()),
                    },
                    LockedPackage::Pypi(pkg) => JsonPackageDiff {
                        name: pkg.name().as_dist_info_name().into_owned(),
                        before: None,
                        after: Some(
                            serde_json::to_value(&pkg).expect("should be able to serialize"),
                        ),
                        ty: JsonPackageType::Pypi,
                        explicit: pypi_dependencies.contains_key(pkg.name()),
                    },
                });

                let removed_diffs = packages_diff.removed.into_iter().map(|old| match old {
                    LockedPackage::Conda(pkg) => JsonPackageDiff {
                        name: pkg.name().as_normalized().to_string(),
                        before: Some(
                            serde_json::to_value(&pkg).expect("should be able to serialize"),
                        ),
                        after: None,
                        ty: JsonPackageType::Conda,
                        explicit: conda_dependencies.contains_key(pkg.name()),
                    },

                    LockedPackage::Pypi(pkg) => JsonPackageDiff {
                        name: pkg.name().as_dist_info_name().into_owned(),
                        before: Some(
                            serde_json::to_value(&pkg).expect("should be able to serialize"),
                        ),
                        after: None,
                        ty: JsonPackageType::Pypi,
                        explicit: pypi_dependencies.contains_key(pkg.name()),
                    },
                });

                let changed_diffs = packages_diff.changed.into_iter().map(|(old, new)| match (old, new) {
                    (LockedPackage::Conda(old), LockedPackage::Conda(new)) =>
                        {
                            let before = serde_json::to_value(&old).expect("should be able to serialize");
                            let after = serde_json::to_value(&new).expect("should be able to serialize");
                            let (before, after) = compute_json_diff(before, after);
                            JsonPackageDiff {
                                name: old.name().as_normalized().to_string(),
                                before: Some(before),
                                after: Some(after),
                                ty: JsonPackageType::Conda,
                                explicit: conda_dependencies.contains_key(old.name()),
                            }
                        }
                    (LockedPackage::Pypi(old), LockedPackage::Pypi(new)) => {
                        let before = serde_json::to_value(&old).expect("should be able to serialize");
                        let after = serde_json::to_value(&new).expect("should be able to serialize");
                        let (before, after) = compute_json_diff(before, after);
                        JsonPackageDiff {
                            name: old.name().as_dist_info_name().into_owned(),
                            before: Some(before),
                            after: Some(after),
                            ty: JsonPackageType::Pypi,
                            explicit: pypi_dependencies.contains_key(old.name()),
                        }
                    }
                    _ => unreachable!("packages cannot change type, they are represented as removals and inserts instead"),
                });

                let packages_diff_json = add_diffs
                    .chain(removed_diffs)
                    .chain(changed_diffs)
                    .sorted_by_key(|diff| diff.name.clone())
                    .collect_vec();

                environment_diff_json.insert(platform.to_string(), packages_diff_json);
            }

            environment.insert(environment_name, environment_diff_json);
        }

        Self {
            version: 1,
            environment,
        }
    }
}

fn compute_json_diff(
    mut a: serde_json::Value,
    mut b: serde_json::Value,
) -> (serde_json::Value, serde_json::Value) {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object_mut()) {
        a.retain(|key, value| {
            if let Some(other_value) = b.get(key) {
                if other_value == value {
                    b.remove(key);
                    return false;
                }
            } else {
                b.insert(key.to_string(), Value::Null);
            }
            true
        });
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pixi_manifest::PixiPlatformName;
    use rattler_conda_types::{
        PackageName, PackageRecord, Platform, Version, package::DistArchiveIdentifier,
    };
    use rattler_lock::{
        CondaBinaryData, CondaPackageData, LockFile, PlatformData, PlatformName, UrlOrPath,
    };
    use url::Url;

    use super::LockFileDiff;

    fn conda_package(url: &str) -> CondaPackageData {
        CondaPackageData::Binary(Box::new(CondaBinaryData {
            package_record: PackageRecord::new(
                PackageName::new_unchecked("foo"),
                Version::from_str("1.0").unwrap(),
                "0".to_string(),
            ),
            location: UrlOrPath::Url(Url::parse(url).unwrap()),
            file_name: DistArchiveIdentifier::try_from_filename("foo-1.0-0.conda").unwrap(),
            channel: None,
        }))
    }

    fn lock_file_with_foreign_platform(url: &str) -> LockFile {
        let mut builder = LockFile::builder()
            // `linux` is valid for rattler but reserved (invalid) for pixi --
            // the mismatch that used to panic the diff.
            .with_platforms(vec![PlatformData {
                name: PlatformName::try_from("linux").unwrap(),
                subdir: Platform::Linux64,
                virtual_packages: vec![],
            }])
            .unwrap();
        builder.set_channels("default", Vec::<rattler_lock::Channel>::new());
        builder.set_options("default", rattler_lock::SolveOptions::default());
        builder
            .add_conda_package("default", "linux", conda_package(url))
            .unwrap();
        builder.finish()
    }

    /// A lock-file platform name that rattler accepts but pixi rejects must be
    /// preserved in the diff, not crash it. Regression test for the
    /// `PixiPlatformName::try_from(...).expect(...)` panics.
    #[test]
    fn diff_preserves_foreign_platform_name() {
        assert!(
            PlatformName::try_from("linux").is_ok(),
            "precondition: rattler accepts `linux` as a platform name"
        );
        assert!(
            PixiPlatformName::try_from("linux").is_err(),
            "precondition: pixi rejects `linux` as a reserved platform name"
        );

        let previous = lock_file_with_foreign_platform("https://example.com/foo-1.0-0.conda");
        let current = lock_file_with_foreign_platform("https://example.com/foo-2.0-0.conda");

        let diff = LockFileDiff::from_lock_files(&previous, &current);

        let platforms = diff
            .environment
            .get("default")
            .expect("the default environment should appear in the diff");
        assert!(
            platforms.contains_key(&PlatformName::try_from("linux").unwrap()),
            "the foreign `linux` platform should be preserved, got {:?}",
            platforms.keys().collect::<Vec<_>>()
        );
    }
}
