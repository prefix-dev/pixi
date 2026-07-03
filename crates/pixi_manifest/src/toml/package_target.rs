use std::path::Path;

use indexmap::IndexMap;
use pixi_build_types::ExtraGroupName;
use pixi_spec::TomlSpec;
use pixi_toml::{Same, TomlIndexMap, TomlWith};
use rattler_conda_types::PackageName;
use toml_span::{DeserError, Value, de_helpers::TableHelper};

use crate::{
    InlinePackageManifest, KnownPreviewFeature, Preview, SpecType, TomlError, Warning,
    WithWarnings,
    error::GenericError,
    target::PackageTarget,
    toml::target::combine_target_dependencies,
    toml::{PackageDefaults, TomlPackage, WorkspacePackageProperties},
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
    /// Converts this target into a [`PackageTarget`].
    ///
    /// `workspace_properties` and `root_directory` are used to convert any
    /// inline package definitions attached to the dependency specs; the
    /// definitions inherit the consuming workspace's package properties, so
    /// `{ workspace = true }` fields resolve as they would for an on-disk
    /// `[package]`.
    pub fn into_package_target(
        self,
        preview: &Preview,
        workspace_dependencies: &IndexMap<PackageName, TomlSpec>,
        workspace_properties: &WorkspacePackageProperties,
        root_directory: &Path,
    ) -> Result<WithWarnings<PackageTarget>, TomlError> {
        let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);
        let mut warnings: Vec<Warning> = Vec::new();

        let TomlPackageTarget {
            mut run_dependencies,
            run_constraints,
            mut host_dependencies,
            mut build_dependencies,
            mut extra_dependencies,
        } = self;

        // Constraints only apply to packages resolved from channels; an inline
        // package definition (which describes how to build a source dependency)
        // is meaningless there.
        if let Some(run_constraints) = &run_constraints
            && let Some((_, package)) = run_constraints.value.inline_packages.first()
        {
            return Err(TomlError::Generic(
                GenericError::new(
                    "inline package definitions are not allowed in `run-constraints`",
                )
                .with_opt_span(package.span.clone())
                .with_span_label("inline package definition specified here")
                .with_help(
                    "constraints only apply to packages resolved from channels, not source packages",
                ),
            ));
        }

        // Peel inline package definitions off each dependency table, leaving
        // the source specs to flow into the regular dependency map. A package
        // name may carry at most one inline definition across the tables of
        // this target.
        let mut inline_toml: IndexMap<PackageName, PixiSpanned<TomlPackage>> = IndexMap::new();
        {
            let mut drain_inline = |map: &mut InheritablePackageMap| -> Result<(), TomlError> {
                for (name, package) in std::mem::take(&mut map.inline_packages) {
                    if inline_toml.insert(name.clone(), package).is_some() {
                        return Err(TomlError::Generic(GenericError::new(format!(
                            "the package '{}' has more than one inline definition",
                            name.as_source()
                        ))));
                    }
                }
                Ok(())
            };

            for table in [
                &mut run_dependencies,
                &mut host_dependencies,
                &mut build_dependencies,
            ]
            .into_iter()
            .flatten()
            {
                drain_inline(&mut table.value)?;
            }
            for table in extra_dependencies.values_mut() {
                drain_inline(&mut table.value)?;
            }
        }

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

        let extra_dependencies = extra_dependencies
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

        // Convert the inline package definitions into full package manifests.
        // Their build source is taken from the surrounding dependency spec, so
        // the converted manifests carry no `build.source` of their own. Package
        // defaults stay empty: an inline definition describes a dependency, not
        // the consuming project.
        let mut inline_packages: IndexMap<PackageName, InlinePackageManifest> = IndexMap::new();
        for (name, package) in inline_toml {
            let WithWarnings {
                value: manifest,
                warnings: mut package_warnings,
            } = package.value.into_manifest(
                workspace_properties.clone(),
                PackageDefaults::default(),
                preview,
                root_directory,
            )?;
            warnings.append(&mut package_warnings);

            let inline = InlinePackageManifest::from_named_manifest(&name, manifest);
            inline_packages.insert(name, inline);
        }

        Ok(WithWarnings::from(PackageTarget {
            dependencies: combine_target_dependencies(
                [
                    (SpecType::Run, resolve(run_dependencies)?),
                    (SpecType::Host, resolve(host_dependencies)?),
                    (SpecType::Build, resolve(build_dependencies)?),
                    (SpecType::RunConstraints, resolve(run_constraints)?),
                ],
                pixi_build_enabled,
            )?,
            extra_dependencies,
            inline_packages,
        })
        .with_warnings(warnings))
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

    fn into_package_target(
        target: TomlPackageTarget,
        preview: &Preview,
    ) -> Result<PackageTarget, TomlError> {
        target
            .into_package_target(
                preview,
                &IndexMap::new(),
                &WorkspacePackageProperties::default(),
                Path::new(""),
            )
            .map(|with_warnings| with_warnings.value)
    }

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

        let package_target = into_package_target(
            TomlPackageTarget::from_toml_str(input).unwrap(),
            &Preview::default(),
        )
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
        let err = into_package_target(
            TomlPackageTarget::from_toml_str(input).unwrap(),
            &Preview::default(),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("extra") && message.contains("invalid character"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn test_inline_package_in_run_dependencies() {
        // An inline package definition on a run dependency is peeled off into
        // the target's inline map; the source spec stays in the dependency
        // table.
        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        let input = r#"
        [run-dependencies]
        rust-package = { git = "https://github.com/user/repo.git", package.build = { backend = { name = "pixi-build-rust", version = "1.0" } } }
        "#;

        let target =
            into_package_target(TomlPackageTarget::from_toml_str(input).unwrap(), &preview)
                .unwrap();

        let name = PackageName::from_str("rust-package").unwrap();
        let spec = target
            .dependencies
            .get(&SpecType::Run)
            .and_then(|d| d.get(&name))
            .and_then(|s| s.iter().next())
            .expect("spec retained");
        assert!(spec.is_source(), "the spec should remain a source spec");

        let inline = target
            .inline_packages
            .get(&name)
            .expect("inline package captured");
        assert_eq!(
            inline.manifest.build.backend.name.as_normalized(),
            "pixi-build-rust"
        );
    }

    #[test]
    fn test_inline_package_in_extra_dependencies() {
        // Extra dependency groups accept source specs, so they accept inline
        // definitions too.
        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        let input = r#"
        [extra-dependencies.test]
        rust-package = { git = "https://github.com/user/repo.git", package.build = { backend = { name = "pixi-build-rust", version = "1.0" } } }
        "#;

        let target =
            into_package_target(TomlPackageTarget::from_toml_str(input).unwrap(), &preview)
                .unwrap();

        let name = PackageName::from_str("rust-package").unwrap();
        assert!(
            target.inline_packages.contains_key(&name),
            "inline definition from an extra group must be captured"
        );
    }

    #[test]
    fn test_inline_package_rejected_in_run_constraints() {
        // Constraints are binary-only; an inline definition there is an error.
        let input = r#"
        [run-constraints]
        rust-package = { git = "https://github.com/user/repo.git", package.build = { backend = { name = "pixi-build-rust", version = "1.0" } } }
        "#;

        let err = into_package_target(
            TomlPackageTarget::from_toml_str(input).unwrap(),
            &Preview::from_iter([KnownPreviewFeature::PixiBuild]),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("run-constraints"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_inline_package_duplicate_across_tables() {
        // The same dependency name may carry at most one inline definition per
        // target, mirroring the workspace-level rule.
        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        let input = r#"
        [run-dependencies]
        rust-package = { git = "https://github.com/user/repo.git", package.build = { backend = { name = "pixi-build-rust", version = "1.0" } } }

        [build-dependencies]
        rust-package = { git = "https://github.com/user/repo.git", package.build = { backend = { name = "pixi-build-rust", version = "1.0" } } }
        "#;

        let err = into_package_target(TomlPackageTarget::from_toml_str(input).unwrap(), &preview)
            .unwrap_err();
        assert!(
            err.to_string().contains("more than one inline definition"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_inline_package_with_workspace_marker_rejected() {
        // `workspace = true` and an inline definition split ownership between
        // the pool and the use site; the combination is rejected at parse time.
        let input = r#"
        [run-dependencies]
        rust-package = { workspace = true, package.build = { backend = { name = "pixi-build-rust", version = "1.0" } } }
        "#;

        let err = TomlPackageTarget::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, err);
        assert!(
            rendered.contains("[workspace.dependencies]"),
            "unexpected error: {rendered}"
        );
    }

    #[test]
    fn test_inline_package_requires_source_location() {
        // An inline definition without a source location is meaningless.
        let input = r#"
        [run-dependencies]
        rust-package = { version = "1.0", package.build = { backend = { name = "pixi-build-rust", version = "1.0" } } }
        "#;

        let err = TomlPackageTarget::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, err);
        assert!(
            rendered.contains("requires a `git`, `path` or `url` source"),
            "unexpected error: {rendered}"
        );
    }
}
