use std::collections::HashMap;

use indexmap::IndexMap;
use pixi_build_types::ExtraGroupName;
use pixi_spec::{PixiSpec, TomlSpec};
use pixi_toml::{Same, TomlIndexMap, TomlWith};
use rattler_conda_types::PackageName;
use toml_span::{DeserError, Value, de_helpers::TableHelper};

use crate::{
    KnownPreviewFlag, PackageDependencySpec, Preview, SpecType, TomlError,
    error::GenericError,
    target::{PackageRunExports, PackageTarget},
    utils::{
        PixiSpanned,
        inheritable_package_map::{InheritablePackageMap, ResolvedPackageMap},
    },
};

#[derive(Debug, Default)]
pub struct TomlPackageTarget {
    pub run_dependencies: Option<PixiSpanned<InheritablePackageMap>>,
    pub run_constraints: Option<PixiSpanned<InheritablePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<InheritablePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<InheritablePackageMap>>,
    pub extra_dependencies: IndexMap<PixiSpanned<String>, PixiSpanned<InheritablePackageMap>>,

    pub run_exports: TomlRunExportsTarget,
}

/// The five run-export buckets of a single package target.
#[derive(Debug, Default)]
pub struct TomlRunExportsTarget {
    pub noarch: Option<PixiSpanned<InheritablePackageMap>>,
    pub strong: Option<PixiSpanned<InheritablePackageMap>>,
    pub weak: Option<PixiSpanned<InheritablePackageMap>>,
    pub strong_constraints: Option<PixiSpanned<InheritablePackageMap>>,
    pub weak_constraints: Option<PixiSpanned<InheritablePackageMap>>,
}

impl TomlRunExportsTarget {
    /// Returns true when no bucket is set.
    pub fn is_empty(&self) -> bool {
        let Self {
            noarch,
            strong,
            weak,
            strong_constraints,
            weak_constraints,
        } = self;
        noarch.is_none()
            && strong.is_none()
            && weak.is_none()
            && strong_constraints.is_none()
            && weak_constraints.is_none()
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlPackageTarget {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let run_dependencies = th.optional("run-dependencies");
        let run_constraints = th.optional("run-constraints");
        let host_dependencies = th.optional("host-dependencies");
        let build_dependencies = th.optional("build-dependencies");
        let extra_dependencies = th
            .optional::<TomlWith<_, TomlIndexMap<_, Same>>>("extra-dependencies")
            .map(TomlWith::into_inner)
            .unwrap_or_default();
        th.finalize(None)?;
        Ok(TomlPackageTarget {
            run_dependencies,
            run_constraints,
            host_dependencies,
            build_dependencies,
            extra_dependencies,
            run_exports: TomlRunExportsTarget::default(),
        })
    }
}

impl TomlPackageTarget {
    pub fn into_package_target(
        self,
        preview: &Preview,
        workspace_dependencies: &IndexMap<PackageName, TomlSpec>,
        package_name: Option<&PackageName>,
    ) -> Result<PackageTarget, TomlError> {
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFlag::PixiBuild);

        let resolve = |entry: Option<PixiSpanned<InheritablePackageMap>>| -> Result<
            Option<ResolvedPackageMap>,
            TomlError,
        > {
            entry
                .map(|spanned| spanned.value.resolve(workspace_dependencies, pixi_build_enabled))
                .transpose()
        };

        // The section matrix: run- and host-dependencies accept
        // `pin-compatible` (they have a previous environment to pin
        // against), while build-dependencies are resolved first and
        // run-constraints only restrict versions. `pin-subpackage` is
        // reserved for the run-export buckets.
        let mut dependencies = HashMap::new();
        if let Some(resolved) = resolve(self.run_dependencies)? {
            let specs =
                resolved.into_dependency_specs("[package.run-dependencies]", package_name)?;
            dependencies.insert(SpecType::Run, specs.into_iter().collect());
        }
        if let Some(resolved) = resolve(self.host_dependencies)? {
            let specs =
                resolved.into_dependency_specs("[package.host-dependencies]", package_name)?;
            dependencies.insert(SpecType::Host, specs.into_iter().collect());
        }
        if let Some(resolved) = resolve(self.build_dependencies)? {
            let specs = resolved.into_pixi_specs(
                "[package.build-dependencies]",
                "The build environment is resolved first, so there is no earlier environment to pin against",
            )?;
            dependencies.insert(
                SpecType::Build,
                specs
                    .into_iter()
                    .map(|(name, spec)| (name, PackageDependencySpec::Spec(spec)))
                    .collect(),
            );
        }
        if let Some(resolved) = resolve(self.run_constraints)? {
            let specs = resolved.into_pixi_specs(
                "[package.run-constraints]",
                "Pins are supported in `[package.run-dependencies]`, `[package.host-dependencies]`, and the `[package.run-exports]` tables",
            )?;
            dependencies.insert(
                SpecType::RunConstraints,
                specs
                    .into_iter()
                    .map(|(name, spec)| (name, PackageDependencySpec::Spec(spec)))
                    .collect(),
            );
        }

        let extra_dependencies = self
            .extra_dependencies
            .into_iter()
            .map(|(name, dependencies)| {
                let PixiSpanned { value: name, span } = name;
                let group = ExtraGroupName::new(name).map_err(|err| {
                    TomlError::Generic(
                        GenericError::new(err.to_string())
                            .with_opt_span(span)
                            .with_span_label("invalid extra dependency group name"),
                    )
                })?;
                let resolved = dependencies
                    .value
                    .resolve(workspace_dependencies, pixi_build_enabled)?;
                let dep_map = resolved
                    .into_pixi_specs(
                        "[package.extra-dependencies]",
                        "Pins are supported in `[package.run-dependencies]`, `[package.host-dependencies]`, and the `[package.run-exports]` tables",
                    )?
                    .into_iter()
                    .collect();
                Ok::<_, TomlError>((group, dep_map))
            })
            .collect::<Result<_, _>>()?;

        let run_exports =
            self.run_exports
                .resolve(workspace_dependencies, pixi_build_enabled, package_name)?;

        Ok(PackageTarget {
            dependencies,
            extra_dependencies,
            run_exports,
        })
    }
}

impl TomlRunExportsTarget {
    /// Resolves the run-export buckets against the workspace dependency pool
    /// and validates the specs.
    ///
    /// Url specs are rejected in every bucket: a run-export lands verbatim in
    /// the built package's metadata, where a url would be meaningless to
    /// consumers. The constraints buckets additionally reject source specs
    /// because a constraint only restricts versions and never pulls a package
    /// in, so there is nothing to build. Both pin kinds are allowed in every
    /// bucket: `pin-subpackage` on the package's own name, `pin-compatible`
    /// on a dependency.
    fn resolve(
        self,
        workspace_dependencies: &IndexMap<PackageName, TomlSpec>,
        pixi_build_enabled: bool,
        package_name: Option<&PackageName>,
    ) -> Result<PackageRunExports, TomlError> {
        let dependency_bucket = |entry| {
            Ok::<_, TomlError>(
                resolve_run_export_bucket(entry, workspace_dependencies, pixi_build_enabled)?
                    .map(|resolved| resolved.into_run_export_specs(package_name))
                    .transpose()?
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            )
        };
        let constraints_bucket = |entry| {
            Ok::<_, TomlError>(
                resolve_run_export_bucket(entry, workspace_dependencies, pixi_build_enabled)?
                    .map(|resolved| resolved.into_run_export_constraints(package_name))
                    .transpose()?
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            )
        };

        Ok(PackageRunExports {
            noarch: dependency_bucket(self.noarch)?,
            strong: dependency_bucket(self.strong)?,
            weak: dependency_bucket(self.weak)?,
            strong_constraints: constraints_bucket(self.strong_constraints)?,
            weak_constraints: constraints_bucket(self.weak_constraints)?,
        })
    }
}

/// Resolves a single run-export bucket against the workspace dependency pool
/// and rejects the specs that never belong in a run-export.
fn resolve_run_export_bucket(
    entry: Option<PixiSpanned<InheritablePackageMap>>,
    workspace_dependencies: &IndexMap<PackageName, TomlSpec>,
    pixi_build_enabled: bool,
) -> Result<Option<ResolvedPackageMap>, TomlError> {
    entry
        .map(|spanned| {
            let resolved = spanned
                .value
                .resolve(workspace_dependencies, pixi_build_enabled)?;
            reject_url_run_exports(&resolved)?;
            Ok::<_, TomlError>(resolved)
        })
        .transpose()
}

/// Rejects url specs and binary path specs in a run-export bucket.
/// Run-exports are recorded in the built package's metadata, where a url would
/// be meaningless to consumers; a path to a package archive would be
/// absolutized into an equally meaningless machine-local file url.
fn reject_url_run_exports(map: &ResolvedPackageMap) -> Result<(), TomlError> {
    for (name, spec) in &map.specs {
        match spec.as_spec() {
            Some(PixiSpec::UrlBinary(_)) | Some(PixiSpec::UrlSource(_)) => {
                return Err(GenericError::new(
                    "url specs are not supported in `[package.run-exports]`",
                )
                .with_opt_span(map.value_spans.get(name).cloned())
                .with_span_label("url spec specified here")
                .with_help("Use a version spec or a `path` or `git` source spec instead")
                .into());
            }
            Some(PixiSpec::PathBinary(_)) => {
                return Err(GenericError::new(
                    "paths to package archives are not supported in `[package.run-exports]`",
                )
                .with_opt_span(map.value_spans.get(name).cloned())
                .with_span_label("package archive path specified here")
                .with_help("Use a version spec or a `path` source spec pointing at a source directory instead")
                .into());
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;
    use rattler_conda_types::PackageName;

    use super::*;
    use crate::toml::FromTomlStr;

    #[test]
    fn test_package_target_all_dependency_types() {
        // All four dependency tables on a package target must end up in the
        // matching SpecType bucket.
        let input = r#"
        [run-dependencies]
        run-dep = "==1.0"

        [host-dependencies]
        host-dep = "==2.0"

        [build-dependencies]
        build-dep = "==3.0"

        [run-constraints]
        constrained = ">=4.0"
        "#;

        let package_target = TomlPackageTarget::from_toml_str(input)
            .unwrap()
            .into_package_target(
                &Preview::default(),
                &IndexMap::new(),
                Some(&PackageName::from_str("mypkg").unwrap()),
            )
            .unwrap();

        let lookup = |spec_type: SpecType, name: &str| -> String {
            package_target
                .dependencies
                .get(&spec_type)
                .and_then(|d| d.get(&PackageName::from_str(name).unwrap()))
                .and_then(|s| s.iter().next())
                .and_then(|s| s.as_spec())
                .and_then(|s| s.as_version_spec())
                .map(|v| v.to_string())
                .unwrap_or_else(|| panic!("missing {name} in {spec_type:?}"))
        };

        assert_eq!(lookup(SpecType::Run, "run-dep"), "==1.0");
        assert_eq!(lookup(SpecType::Host, "host-dep"), "==2.0");
        assert_eq!(lookup(SpecType::Build, "build-dep"), "==3.0");
        assert_eq!(lookup(SpecType::RunConstraints, "constrained"), ">=4.0");
    }

    #[test]
    fn test_package_target_unknown_key() {
        // A typo like `run-constraint` (singular) must be flagged so users
        // don't silently lose their constraints.
        let input = r#"
        [run-constraint]
        oops = "==1.0"
        "#;
        let err = TomlPackageTarget::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, err));
    }

    #[test]
    fn test_invalid_extra_group_name_is_rejected() {
        // Extra group names follow the extras naming
        // scheme `^[a-z0-9._+-]{1,64}$`; an uppercase name is rejected with a
        // spanned error rather than silently producing invalid v3 metadata.
        let input = r#"
        [extra-dependencies.Invalid]
        gtest = "*"
        "#;
        let err = TomlPackageTarget::from_toml_str(input)
            .unwrap()
            .into_package_target(
                &Preview::default(),
                &IndexMap::new(),
                Some(&PackageName::from_str("mypkg").unwrap()),
            )
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("extra") && message.contains("invalid character"),
            "unexpected error: {message}"
        );
    }
}
