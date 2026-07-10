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
    utils::{
        PixiSpanned,
        inheritable_package_map::{InheritablePackageMap, InheritableSpec},
    },
};

#[derive(Debug, Default, Clone)]
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
            } = package.value.into_inline_manifest(
                workspace_properties.clone(),
                PackageDefaults::default(),
                preview,
                root_directory,
            )?;
            warnings.append(&mut package_warnings);

            let inline = InlinePackageManifest::from_named_manifest(&name, manifest);
            inline_packages.insert(name, inline);
        }

        // `{ workspace = true }` entries inherit the pool's inline definition
        // together with the spec: a manifest-less source location without its
        // definition would fail discovery at build time. The same pool entry
        // inherited in several tables is fine (identical content); clashing
        // with a direct definition of the same name is not.
        {
            let mut adopt = |map: &InheritablePackageMap| -> Result<(), TomlError> {
                for (name, spec) in &map.specs {
                    if !matches!(spec, InheritableSpec::Inherited { .. }) {
                        continue;
                    }
                    let Some(pool_inline) =
                        workspace_properties.dependency_inline_packages.get(name)
                    else {
                        continue;
                    };
                    match inline_packages.entry(name.clone()) {
                        indexmap::map::Entry::Occupied(existing) => {
                            if existing.get().content_hash != pool_inline.content_hash {
                                return Err(TomlError::Generic(GenericError::new(format!(
                                    "the package '{}' has more than one inline definition",
                                    name.as_source()
                                ))));
                            }
                        }
                        indexmap::map::Entry::Vacant(entry) => {
                            entry.insert(pool_inline.clone());
                        }
                    }
                }
                Ok(())
            };
            for table in [&run_dependencies, &host_dependencies, &build_dependencies]
                .into_iter()
                .flatten()
            {
                adopt(&table.value)?;
            }
            for table in extra_dependencies.values() {
                adopt(&table.value)?;
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
    use crate::toml::TomlWorkspace;

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

    /// Build workspace properties whose pool declares a single git source
    /// dependency `name` carrying an inline definition.
    fn pool_properties_with_inline(name: &str) -> WorkspacePackageProperties {
        let doc = format!(
            r#"
            name = "ws"
            channels = []
            platforms = []
            preview = ["pixi-build"]

            [dependencies]
            {name} = {{ git = "https://github.com/user/repo.git", package.build = {{ backend = {{ name = "pixi-build-rust", version = "1.0" }} }} }}
            "#
        );
        let workspace = TomlWorkspace::from_toml_str(&doc)
            .expect("workspace parses")
            .into_workspace(Default::default(), Path::new(""))
            .expect("workspace converts")
            .value;
        WorkspacePackageProperties {
            dependencies: workspace.dependencies,
            dependency_inline_packages: workspace.dependency_inline_packages,
            ..Default::default()
        }
    }

    #[test]
    fn test_workspace_marker_inherits_pool_inline_definition() {
        // `{ workspace = true }` inherits the pool entry's inline definition
        // together with the source spec.
        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        let properties = pool_properties_with_inline("rust-package");
        let input = r#"
        [run-dependencies]
        rust-package = { workspace = true }
        "#;

        let target = TomlPackageTarget::from_toml_str(input)
            .unwrap()
            .into_package_target(
                &preview,
                &properties.dependencies.clone(),
                &properties,
                Path::new(""),
            )
            .unwrap()
            .value;

        let name = PackageName::from_str("rust-package").unwrap();
        let inline = target
            .inline_packages
            .get(&name)
            .expect("pool inline definition inherited");
        assert_eq!(
            inline.manifest.build.backend.name.as_normalized(),
            "pixi-build-rust"
        );
        let spec = target
            .dependencies
            .get(&SpecType::Run)
            .and_then(|d| d.get(&name))
            .and_then(|s| s.iter().next())
            .expect("spec inherited");
        assert!(spec.is_source(), "the inherited spec is a source spec");
    }

    #[test]
    fn test_workspace_marker_inherited_twice_is_not_a_conflict() {
        // The same pool definition inherited in two tables has identical
        // content and must not trip the duplicate check.
        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        let properties = pool_properties_with_inline("rust-package");
        let input = r#"
        [run-dependencies]
        rust-package = { workspace = true }

        [host-dependencies]
        rust-package = { workspace = true }
        "#;

        let target = TomlPackageTarget::from_toml_str(input)
            .unwrap()
            .into_package_target(
                &preview,
                &properties.dependencies.clone(),
                &properties,
                Path::new(""),
            )
            .unwrap()
            .value;
        assert_eq!(target.inline_packages.len(), 1);
    }

    #[test]
    fn test_direct_definition_conflicts_with_inherited_pool_definition() {
        // A direct inline definition in one table and a pool-inherited one in
        // another for the same name disagree about the package's content.
        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        let properties = pool_properties_with_inline("rust-package");
        let input = r#"
        [run-dependencies]
        rust-package = { workspace = true }

        [build-dependencies]
        rust-package = { git = "https://github.com/user/other.git", package.build = { backend = { name = "pixi-build-cmake", version = "1.0" } } }
        "#;

        let err = TomlPackageTarget::from_toml_str(input)
            .unwrap()
            .into_package_target(
                &preview,
                &properties.dependencies.clone(),
                &properties,
                Path::new(""),
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("more than one inline definition"),
            "unexpected error: {err}"
        );
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
    fn test_inline_package_in_host_and_build_dependencies() {
        // The host and build dependency tables peel inline definitions like
        // run-dependencies does.
        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        let input = r#"
        [host-dependencies]
        host-tool = { path = "host_pkg", package.build = { backend = { name = "pixi-build-rattler-build", version = "*" } } }

        [build-dependencies]
        build-tool = { path = "build_pkg", package.build = { backend = { name = "pixi-build-rattler-build", version = "*" } } }
        "#;

        let target =
            into_package_target(TomlPackageTarget::from_toml_str(input).unwrap(), &preview)
                .unwrap();
        for name in ["host-tool", "build-tool"] {
            let name = PackageName::from_str(name).unwrap();
            assert!(
                target.inline_packages.contains_key(&name),
                "inline definition of '{}' captured",
                name.as_source()
            );
        }
    }

    /// Build workspace properties whose pool declares a single binary
    /// dependency `name`.
    fn pool_properties_with_binary(name: &str) -> WorkspacePackageProperties {
        let doc = format!(
            r#"
            name = "ws"
            channels = []
            platforms = []

            [dependencies]
            {name} = ">=1"
            "#
        );
        let workspace = TomlWorkspace::from_toml_str(&doc)
            .expect("workspace parses")
            .into_workspace(Default::default(), Path::new(""))
            .expect("workspace converts")
            .value;
        WorkspacePackageProperties {
            dependencies: workspace.dependencies,
            dependency_inline_packages: workspace.dependency_inline_packages,
            ..Default::default()
        }
    }

    #[test]
    fn test_workspace_marker_on_binary_pool_entry() {
        // A plain binary pool entry inherited with `{ workspace = true }`
        // stays a binary dependency without any inline definition.
        let properties = pool_properties_with_binary("zlib");
        let input = r#"
        [run-dependencies]
        zlib = { workspace = true }
        "#;

        let target = TomlPackageTarget::from_toml_str(input)
            .unwrap()
            .into_package_target(
                &Preview::default(),
                &properties.dependencies.clone(),
                &properties,
                Path::new(""),
            )
            .unwrap()
            .value;

        let name = PackageName::from_str("zlib").unwrap();
        assert!(
            target.inline_packages.is_empty(),
            "no inline definition must be inherited from a binary pool entry"
        );
        let spec = target
            .dependencies
            .get(&SpecType::Run)
            .and_then(|d| d.get(&name))
            .and_then(|s| s.iter().next())
            .expect("spec inherited");
        assert!(spec.is_binary(), "the inherited spec stays binary");
    }

    #[test]
    fn test_workspace_marker_on_source_pool_entry_without_definition() {
        // A source pool entry without an inline definition is inherited as a
        // plain source dependency; discovery will use the on-disk manifest.
        let doc = r#"
            name = "ws"
            channels = []
            platforms = []
            preview = ["pixi-build"]

            [dependencies]
            tool-c = { path = "c_pkg" }
            "#;
        let workspace = TomlWorkspace::from_toml_str(doc)
            .expect("workspace parses")
            .into_workspace(Default::default(), Path::new(""))
            .expect("workspace converts")
            .value;
        let properties = WorkspacePackageProperties {
            dependencies: workspace.dependencies,
            dependency_inline_packages: workspace.dependency_inline_packages,
            ..Default::default()
        };

        let input = r#"
        [run-dependencies]
        tool-c = { workspace = true }
        "#;
        let target = TomlPackageTarget::from_toml_str(input)
            .unwrap()
            .into_package_target(
                &Preview::from_iter([KnownPreviewFeature::PixiBuild]),
                &properties.dependencies.clone(),
                &properties,
                Path::new(""),
            )
            .unwrap()
            .value;

        let name = PackageName::from_str("tool-c").unwrap();
        assert!(
            target.inline_packages.is_empty(),
            "no inline definition to inherit"
        );
        let spec = target
            .dependencies
            .get(&SpecType::Run)
            .and_then(|d| d.get(&name))
            .and_then(|s| s.iter().next())
            .expect("spec inherited");
        assert!(spec.is_source(), "the inherited spec stays a source spec");
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
