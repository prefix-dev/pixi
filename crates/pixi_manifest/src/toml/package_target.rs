use indexmap::IndexMap;
use pixi_build_types::ExtraGroupName;
use pixi_spec::TomlSpec;
use pixi_toml::{Same, TomlIndexMap, TomlWith};
use rattler_conda_types::PackageName;
use toml_span::{DeserError, Value, de_helpers::TableHelper};

use crate::{
    KnownPreviewFeature, Preview, SpecType, TomlError,
    error::GenericError,
    target::PackageTarget,
    toml::target::combine_target_dependencies,
    utils::{PixiSpanned, inheritable_package_map::InheritablePackageMap},
};

#[derive(Debug, Default)]
pub struct TomlPackageTarget {
    pub run_dependencies: Option<PixiSpanned<InheritablePackageMap>>,
    pub run_constraints: Option<PixiSpanned<InheritablePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<InheritablePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<InheritablePackageMap>>,
    pub extra_dependencies: IndexMap<PixiSpanned<String>, PixiSpanned<InheritablePackageMap>>,
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
        })
    }
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
