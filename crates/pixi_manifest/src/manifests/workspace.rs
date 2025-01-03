use std::hash::Hash;

use indexmap::{map::IndexMap, Equivalent};

use crate::toml::FromTomlStr;
use crate::{
    consts,
    environment::{Environment, EnvironmentName},
    environments::Environments,
    feature::{Feature, FeatureName},
    solve_group::SolveGroups,
    toml::ExternalWorkspaceProperties,
    workspace::Workspace,
    TomlError,
};

/// Holds the parsed content of the workspace part of a pixi manifest. This
/// describes the part related to the workspace only.
#[derive(Debug, Clone)]
pub struct WorkspaceManifest {
    /// Information about the project
    pub workspace: Workspace,

    /// All the features defined in the project.
    pub features: IndexMap<FeatureName, Feature>,

    /// All the environments defined in the project.
    pub environments: Environments,

    /// The solve groups that are part of the project.
    pub solve_groups: SolveGroups,
}

impl WorkspaceManifest {
    /// Parses a TOML string into a `WorkspaceManifest`.
    pub fn from_toml_str(toml_str: &str) -> Result<Self, TomlError> {
        let manifest = crate::toml::TomlManifest::from_toml_str(toml_str)?;
        Ok(manifest
            .into_manifests(ExternalWorkspaceProperties::default())?
            .0)
    }

    /// Returns the default feature.
    ///
    /// This is the feature that is added implicitly by the tables at the root
    /// of the project manifest.
    pub fn default_feature(&self) -> &Feature {
        self.features
            .get(&FeatureName::Default)
            .expect("default feature should always exist")
    }

    /// Returns a mutable reference to the default feature.
    pub(crate) fn default_feature_mut(&mut self) -> &mut Feature {
        self.features
            .get_mut(&FeatureName::Default)
            .expect("default feature should always exist")
    }

    /// Returns the default environment
    ///
    /// This is the environment that is added implicitly as the environment with
    /// only the default feature. The default environment can be overwritten
    /// by a environment named `default`.
    pub fn default_environment(&self) -> &Environment {
        let envs = &self.environments;
        envs.find(&EnvironmentName::Named(String::from(
            consts::DEFAULT_ENVIRONMENT_NAME,
        )))
        .or_else(|| envs.find(&EnvironmentName::Default))
        .expect("default environment should always exist")
    }

    /// Returns the environment with the given name or `None` if it does not
    /// exist.
    pub fn environment<Q>(&self, name: &Q) -> Option<&Environment>
    where
        Q: ?Sized + Hash + Equivalent<EnvironmentName>,
    {
        self.environments.find(name)
    }
}

#[cfg(test)]
mod tests {
    use insta::{assert_debug_snapshot, assert_snapshot, assert_yaml_snapshot};
    use itertools::Itertools;
    use rattler_conda_types::{NamedChannelOrUrl, Platform};

    use crate::utils::test_utils::format_parse_error;
    use crate::{utils::test_utils::expect_parse_failure, TargetSelector, WorkspaceManifest};

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = []
        "#;

    #[test]
    fn test_target_specific() {
        let contents = format!(
            r#"
        {PROJECT_BOILERPLATE}

        [target.win-64.dependencies]
        foo = "3.4.5"

        [target.osx-64.dependencies]
        foo = "1.2.3"
        "#
        );

        let manifest = WorkspaceManifest::from_toml_str(&contents).unwrap();
        let targets = &manifest.default_feature().targets;
        assert_eq!(
            targets.user_defined_selectors().cloned().collect_vec(),
            vec![
                TargetSelector::Platform(Platform::Win64),
                TargetSelector::Platform(Platform::Osx64),
            ]
        );

        let win64_target = targets
            .for_target(&TargetSelector::Platform(Platform::Win64))
            .unwrap();
        let osx64_target = targets
            .for_target(&TargetSelector::Platform(Platform::Osx64))
            .unwrap();
        assert_eq!(
            win64_target
                .run_dependencies()
                .unwrap()
                .get("foo")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "==3.4.5"
        );
        assert_eq!(
            osx64_target
                .run_dependencies()
                .unwrap()
                .get("foo")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "==1.2.3"
        );
    }

    #[test]
    fn test_mapped_dependencies() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [dependencies]
            test_map = {{ version = ">=1.2.3", channel="conda-forge", build="py34_0" }}
            test_build = {{ build = "bla" }}
            test_channel = {{ channel = "conda-forge" }}
            test_version = {{ version = ">=1.2.3" }}
            test_version_channel = {{ version = ">=1.2.3", channel = "conda-forge" }}
            test_version_build = {{ version = ">=1.2.3", build = "py34_0" }}
            "#
        );

        let manifest = WorkspaceManifest::from_toml_str(&contents).unwrap();
        let deps = manifest
            .default_feature()
            .targets
            .default()
            .run_dependencies()
            .unwrap();
        let test_map_spec = deps.get("test_map").unwrap().as_detailed().unwrap();

        assert_eq!(
            test_map_spec.version.as_ref().unwrap().to_string(),
            ">=1.2.3"
        );
        assert_eq!(test_map_spec.build.as_ref().unwrap().to_string(), "py34_0");
        assert_eq!(
            test_map_spec.channel,
            Some(NamedChannelOrUrl::Name("conda-forge".to_string()))
        );

        assert_eq!(
            deps.get("test_build")
                .unwrap()
                .as_detailed()
                .unwrap()
                .build
                .as_ref()
                .unwrap()
                .to_string(),
            "bla"
        );

        let test_channel = deps.get("test_channel").unwrap().as_detailed().unwrap();
        assert_eq!(
            test_channel.channel,
            Some(NamedChannelOrUrl::Name("conda-forge".to_string()))
        );

        let test_version = deps.get("test_version").unwrap().as_detailed().unwrap();
        assert_eq!(
            test_version.version.as_ref().unwrap().to_string(),
            ">=1.2.3"
        );

        let test_version_channel = deps
            .get("test_version_channel")
            .unwrap()
            .as_detailed()
            .unwrap();
        assert_eq!(
            test_version_channel.version.as_ref().unwrap().to_string(),
            ">=1.2.3"
        );
        assert_eq!(
            test_version_channel.channel,
            Some(NamedChannelOrUrl::Name("conda-forge".to_string()))
        );

        let test_version_build = deps
            .get("test_version_build")
            .unwrap()
            .as_detailed()
            .unwrap();
        assert_eq!(
            test_version_build.version.as_ref().unwrap().to_string(),
            ">=1.2.3"
        );
        assert_eq!(
            test_version_build.build.as_ref().unwrap().to_string(),
            "py34_0"
        );
    }

    #[test]
    fn test_dependency_types() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [dependencies]
            my-game = "1.0.0"

            [build-dependencies]
            cmake = "*"

            [host-dependencies]
            sdl2 = "*"
            "#
        );

        let manifest = WorkspaceManifest::from_toml_str(&contents).unwrap();
        let default_target = manifest.default_feature().targets.default();
        let run_dependencies = default_target.run_dependencies().unwrap();
        let build_dependencies = default_target.build_dependencies().unwrap();
        let host_dependencies = default_target.host_dependencies().unwrap();

        assert_eq!(
            run_dependencies
                .get("my-game")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "==1.0.0"
        );
        assert_eq!(
            build_dependencies
                .get("cmake")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "*"
        );
        assert_eq!(
            host_dependencies
                .get("sdl2")
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            "*"
        );
    }

    #[test]
    fn test_invalid_target_specific() {
        let examples = [r#"[target.foobar.dependencies]
            invalid_platform = "henk""#];

        assert_snapshot!(expect_parse_failure(&format!(
            "{PROJECT_BOILERPLATE}\n{}",
            examples[0]
        )));
    }

    #[test]
    fn test_invalid_key() {
        insta::with_settings!({snapshot_suffix => "foobar"}, {
            assert_snapshot!(expect_parse_failure(&format!("{PROJECT_BOILERPLATE}\n[foobar]")))
        });

        insta::with_settings!({snapshot_suffix => "hostdependencies"}, {
            assert_snapshot!(expect_parse_failure(&format!("{PROJECT_BOILERPLATE}\n[target.win-64.hostdependencies]")))
        });

        insta::with_settings!({snapshot_suffix => "environment"}, {
            assert_snapshot!(expect_parse_failure(&format!("{PROJECT_BOILERPLATE}\n[environments.INVALID]")))
        });
    }

    #[test]
    fn test_target_specific_tasks() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [tasks]
            test = "test multi"

            [target.win-64.tasks]
            test = "test win"

            [target.linux-64.tasks]
            test = "test linux"
            "#
        );

        let manifest = WorkspaceManifest::from_toml_str(&contents).unwrap();

        assert_snapshot!(manifest
            .default_feature()
            .targets
            .iter()
            .flat_map(|(target, selector)| {
                let selector_name =
                    selector.map_or_else(|| String::from("default"), ToString::to_string);
                target.tasks.iter().filter_map(move |(name, task)| {
                    Some(format!(
                        "{}/{} = {}",
                        &selector_name,
                        name.as_str(),
                        task.as_single_command()?
                    ))
                })
            })
            .join("\n"));
    }

    #[test]
    fn test_python_dependencies() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [pypi-dependencies]
            foo = ">=3.12"
            bar = {{ version=">=3.12", extras=["baz"] }}
            "#
        );

        assert_snapshot!(WorkspaceManifest::from_toml_str(&contents)
            .map_err(|e| format_parse_error(&contents, e))
            .expect("parsing should succeed!")
            .default_feature()
            .targets
            .default()
            .pypi_dependencies
            .clone()
            .into_iter()
            .flat_map(|d| d.into_iter())
            .map(|(name, spec)| format!("{} = {}", name.as_source(), toml_edit::Value::from(spec)))
            .join("\n"));
    }

    #[test]
    fn test_pypi_options_default_feature() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [project.pypi-options]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            [[project.pypi-options.find-links]]
            path = "../foo"
            [[project.pypi-options.find-links]]
            url = "https://example.com/bar"
            "#
        );

        assert_yaml_snapshot!(WorkspaceManifest::from_toml_str(&contents)
            .expect("parsing should succeed!")
            .workspace
            .pypi_options
            .clone()
            .unwrap());
    }

    #[test]
    fn test_pypy_options_project_and_default_feature() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [project.pypi-options]
            extra-index-urls = ["https://pypi.org/simple2"]

            [pypi-options]
            extra-index-urls = ["https://pypi.org/simple3"]
            "#
        );

        let manifest =
            WorkspaceManifest::from_toml_str(&contents).expect("parsing should succeed!");
        assert_yaml_snapshot!(manifest.workspace.pypi_options.clone().unwrap());
    }

    #[test]
    fn test_tool_deserialization() {
        let contents = r#"
        [project]
        name = "foo"
        channels = []
        platforms = []
        [tool.ruff]
        test = "test"
        test1 = ["test"]
        test2 = { test = "test" }

        [tool.ruff.test3]
        test = "test"

        [tool.poetry]
        test = "test"
        "#;
        let _manifest = WorkspaceManifest::from_toml_str(contents).unwrap();
    }

    #[test]
    fn test_build_variants() {
        let contents = r#"
        [workspace]
        name = "foo"
        channels = []
        platforms = []

        [workspace.build-variants]
        python = ["3.10.*", "3.11.*"]

        [workspace.target.win-64.build-variants]
        python = ["1.0.*"]
        "#;
        let manifest = WorkspaceManifest::from_toml_str(contents).unwrap();
        println!("{:?}", manifest.workspace.build_variants);
        let resolved_linux = manifest
            .workspace
            .build_variants
            .resolve(Some(Platform::Linux64))
            .collect::<Vec<_>>();
        assert_debug_snapshot!(resolved_linux);

        let resolved_win = manifest
            .workspace
            .build_variants
            .resolve(Some(Platform::Win64))
            .collect::<Vec<_>>();
        assert_debug_snapshot!(resolved_win);
    }
}
