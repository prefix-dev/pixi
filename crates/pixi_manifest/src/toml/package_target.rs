use indexmap::IndexMap;
use itertools::Either;
use pixi_build_types::ExtraGroupName;
use pixi_spec::{BinarySpec, PixiSpec, TomlSpec};
use pixi_spec_containers::DependencyMap;
use pixi_toml::{Same, TomlIndexMap, TomlWith};
use rattler_conda_types::PackageName;
use toml_span::{DeserError, Value, de_helpers::TableHelper};

use crate::{
    KnownPreviewFeature, Preview, SpecType, TomlError,
    error::GenericError,
    target::{PackageRunExports, PackageTarget},
    toml::target::combine_target_dependencies,
    utils::{
        PixiSpanned, inheritable_package_map::InheritablePackageMap, package_map::UniquePackageMap,
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
    ) -> Result<PackageTarget, TomlError> {
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);

        let resolve = |entry: Option<PixiSpanned<InheritablePackageMap>>| -> Result<
            Option<PixiSpanned<crate::utils::package_map::UniquePackageMap>>,
            TomlError,
        > {
            entry
                .map(|spanned| {
                    let PixiSpanned { value, span } = spanned;
                    let resolved = value.resolve(workspace_dependencies, pixi_build_enabled)?;
                    Ok::<_, TomlError>(PixiSpanned {
                        value: resolved,
                        span,
                    })
                })
                .transpose()
        };

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
                    .into_inner(pixi_build_enabled)?
                    .into_iter()
                    .collect();
                Ok::<_, TomlError>((group, dep_map))
            })
            .collect::<Result<_, _>>()?;

        let run_exports = self
            .run_exports
            .resolve(workspace_dependencies, pixi_build_enabled)?;

        Ok(PackageTarget {
            dependencies: combine_target_dependencies(
                [
                    (SpecType::Run, resolve(self.run_dependencies)?),
                    (SpecType::Host, resolve(self.host_dependencies)?),
                    (SpecType::Build, resolve(self.build_dependencies)?),
                    (SpecType::RunConstraints, resolve(self.run_constraints)?),
                ],
                pixi_build_enabled,
            )?,
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
    /// in, so there is nothing to build.
    fn resolve(
        self,
        workspace_dependencies: &IndexMap<PackageName, TomlSpec>,
        pixi_build_enabled: bool,
    ) -> Result<PackageRunExports, TomlError> {
        let resolve_bucket = |entry: Option<PixiSpanned<InheritablePackageMap>>| -> Result<
            Option<UniquePackageMap>,
            TomlError,
        > {
            entry
                .map(|spanned| {
                    let resolved = spanned
                        .value
                        .resolve(workspace_dependencies, pixi_build_enabled)?;
                    reject_url_run_exports(&resolved)?;
                    Ok::<_, TomlError>(resolved)
                })
                .transpose()
        };

        let dependency_bucket = |entry: Option<PixiSpanned<InheritablePackageMap>>| -> Result<
            DependencyMap<PackageName, PixiSpec>,
            TomlError,
        > {
            Ok(resolve_bucket(entry)?
                .map(|resolved| resolved.into_inner(pixi_build_enabled))
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .collect())
        };

        let constraints_bucket = |entry: Option<PixiSpanned<InheritablePackageMap>>| -> Result<
            DependencyMap<PackageName, BinarySpec>,
            TomlError,
        > {
            let Some(resolved) = resolve_bucket(entry)? else {
                return Ok(DependencyMap::default());
            };
            let mut out = DependencyMap::default();
            for (name, spec) in &resolved.specs {
                match spec.clone().into_source_or_binary() {
                    Either::Right(binary) => {
                        out.insert(name.clone(), binary);
                    }
                    Either::Left(_) => {
                        return Err(GenericError::new(
                            "source specs are not supported in run-export constraints",
                        )
                        .with_opt_span(resolved.value_spans.get(name).cloned())
                        .with_span_label("source spec specified here")
                        .with_help(
                            "A constraint only restricts the version of a package that is installed for another reason; use a version spec instead",
                        )
                        .into());
                    }
                }
            }
            Ok(out)
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

/// Rejects url specs and binary path specs in a run-export bucket.
/// Run-exports are recorded in the built package's metadata, where a url would
/// be meaningless to consumers; a path to a package archive would be
/// absolutized into an equally meaningless machine-local file url.
fn reject_url_run_exports(map: &UniquePackageMap) -> Result<(), TomlError> {
    for (name, spec) in &map.specs {
        match spec {
            PixiSpec::UrlBinary(_) | PixiSpec::UrlSource(_) => {
                return Err(GenericError::new(
                    "url specs are not supported in `[package.run-exports]`",
                )
                .with_opt_span(map.value_spans.get(name).cloned())
                .with_span_label("url spec specified here")
                .with_help("Use a version spec or a `path` or `git` source spec instead")
                .into());
            }
            PixiSpec::PathBinary(_) => {
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
            .into_package_target(&Preview::default(), &IndexMap::new())
            .unwrap();

        let lookup = |spec_type: SpecType, name: &str| -> String {
            package_target
                .dependencies
                .get(&spec_type)
                .and_then(|d| d.get(&PackageName::from_str(name).unwrap()))
                .and_then(|s| s.iter().next())
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
            .into_package_target(&Preview::default(), &IndexMap::new())
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("extra") && message.contains("invalid character"),
            "unexpected error: {message}"
        );
    }
}
