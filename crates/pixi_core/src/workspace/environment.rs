use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    hash::{Hash, Hasher},
    sync::Once,
};

use indexmap::IndexMap;
use itertools::Either;
use pixi_consts::consts;
use pixi_manifest::{
    self as manifest, EnvironmentName, Feature, FeatureName, FeaturesExt, HasFeaturesIter,
    HasWorkspaceManifest, SystemRequirements, Task, TaskName, WorkspaceManifest,
};
use rattler_conda_types::{Arch, ChannelConfig, Platform};

use super::{
    SolveGroup,
    errors::{UnknownTask, UnsupportedPlatformError},
};
use crate::{Workspace, workspace::HasWorkspaceRef};

/// Describes a single environment from a project manifest. This is used to
/// describe environments that can be installed and activated.
///
/// This struct is a higher level representation of a [`manifest::Environment`].
/// The `manifest::Environment` describes the data stored in the manifest file,
/// while this struct provides methods to easily interact with an environment
/// without having to deal with the structure of the project model.
///
/// This type does not provide manipulation methods. To modify the data model
/// you should directly interact with the manifest instead.
///
/// The lifetime `'p` refers to the lifetime of the project that this
/// environment belongs to.
#[derive(Clone)]
pub struct Environment<'p> {
    /// The project this environment belongs to.
    pub(super) workspace: &'p Workspace,

    /// The environment that this environment is based on.
    pub(super) environment: &'p manifest::Environment,
}

impl Debug for Environment<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("project", &self.workspace.display_name())
            .field("environment", &self.environment.name)
            .finish()
    }
}

impl PartialEq for Environment<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.workspace, other.workspace)
            && std::ptr::eq(self.environment, other.environment)
    }
}

impl Eq for Environment<'_> {}

impl<'p> Environment<'p> {
    /// Return new instance of Environment
    pub(crate) fn new(project: &'p Workspace, environment: &'p manifest::Environment) -> Self {
        Self {
            workspace: project,
            environment,
        }
    }

    /// Returns true if this environment is the default environment.
    pub fn is_default(&self) -> bool {
        self.environment.name == EnvironmentName::Default
    }

    /// Returns true if the environment has no_default_feature set to true
    pub fn no_default_feature(&self) -> bool {
        self.environment.no_default_feature
    }

    /// Returns the name of this environment.
    pub fn name(&self) -> &'p EnvironmentName {
        &self.environment.name
    }

    /// Returns the solve group to which this environment belongs, or `None` if
    /// no solve group was specified.
    pub fn solve_group(&self) -> Option<SolveGroup<'p>> {
        self.environment
            .solve_group
            .map(|solve_group_idx| SolveGroup {
                workspace: self.workspace,
                solve_group: &self.workspace.workspace.value.solve_groups[solve_group_idx],
            })
    }

    /// Returns the directory where this environment is stored.
    pub fn dir(&self) -> std::path::PathBuf {
        self.workspace
            .environments_dir()
            .join(self.environment.name.as_str())
    }

    /// We store a hash of the lockfile and all activation env variables in a
    /// file in the cache. The current name is
    /// `activation_environment-name.json`.
    pub fn activation_cache_name(&self) -> String {
        format!("activation_{}.json", self.name())
    }

    /// Returns the activation cache file path.
    pub fn activation_cache_file_path(&self) -> std::path::PathBuf {
        self.workspace
            .activation_env_cache_folder()
            .join(self.activation_cache_name())
    }

    /// Returns the best platform for the current platform & environment.
    pub fn best_platform(&self) -> Platform {
        let current = Platform::current();

        // If the current platform is supported, return it.
        if self.platforms().contains(&current) {
            return current;
        }

        static WARN_ONCE: Once = Once::new();

        // If the current platform is osx-arm64 and the environment supports osx-64,
        // return osx-64.
        if current.is_osx() && self.platforms().contains(&Platform::Osx64) {
            WARN_ONCE.call_once(|| {
                let warn_folder = self.workspace.pixi_dir().join(consts::ONE_TIME_MESSAGES_DIR);
                let emulation_warn = warn_folder.join("macos-emulation-warn");
                if !emulation_warn.exists() {
                    tracing::warn!(
                        "osx-arm64 (Apple Silicon) is not supported by the pixi.toml, falling back to osx-64 (emulated with Rosetta)"
                    );
                    // Create a file to prevent the warning from showing up multiple times. Also ignore the result.
                    fs_err::create_dir_all(warn_folder).and_then(|_| {
                        fs_err::File::create(emulation_warn)
                    }).ok();
                }
            });
            return Platform::Osx64;
        }

        // If the current platform is win-arm64 and the environment supports win-64,
        // return win-64.
        if current.is_windows() && self.platforms().contains(&Platform::Win64) {
            WARN_ONCE.call_once(|| {
                let warn_folder = self.workspace.pixi_dir().join(consts::ONE_TIME_MESSAGES_DIR);
                let emulation_warn = warn_folder.join("windows-emulation-warn");
                if !emulation_warn.exists() {
                    tracing::warn!(
                        "win-arm64 is not supported by the pixi.toml, falling back to win-64 (emulation)"
                    );
                    // Create a file to prevent the warning from showing up multiple times. Also ignore the result.
                    fs_err::create_dir_all(warn_folder).and_then(|_| {
                        fs_err::File::create(emulation_warn)
                    }).ok();
                }
            });
            return Platform::Win64;
        }

        if self.platforms().len() == 1 {
            // Take the first platform and see if it is a WASM one.
            if let Some(platform) = self.platforms().iter().next() {
                if platform.arch() == Some(Arch::Wasm32) {
                    return *platform;
                }
            }
        }

        current
    }

    /// Returns the tasks defined for this environment.
    ///
    /// Tasks are defined on a per-target per-feature per-environment basis.
    ///
    /// If a `platform` is specified but this environment doesn't support the
    /// specified platform, an [`UnsupportedPlatformError`] error is
    /// returned.
    pub fn tasks(
        &self,
        platform: Option<Platform>,
    ) -> Result<IndexMap<&'p TaskName, &'p Task>, UnsupportedPlatformError> {
        self.validate_platform_support(platform)?;
        let result = self
            .features()
            .flat_map(|feature| feature.targets.resolve(platform))
            .rev() // Reverse to get the most specific targets last.
            .flat_map(|target| target.tasks.iter())
            .collect();
        Ok(result)
    }

    /// Return all tasks available for the given environment
    /// This will not return task prefixed with _
    pub fn get_filtered_tasks(&self) -> HashSet<TaskName> {
        self.tasks(Some(self.best_platform()))
            .into_iter()
            .flat_map(|tasks| {
                tasks.into_iter().filter_map(|(key, _)| {
                    if !key.as_str().starts_with('_') {
                        Some(key)
                    } else {
                        None
                    }
                })
            })
            .map(ToOwned::to_owned)
            .collect()
    }
    /// Returns the task with the given `name` and for the specified `platform`
    /// or an `UnknownTask` which explains why the task was not available.
    pub fn task(
        &self,
        name: &TaskName,
        platform: Option<Platform>,
    ) -> Result<&'p Task, UnknownTask> {
        match self.tasks(platform).map(|tasks| tasks.get(name).copied()) {
            Err(_) | Ok(None) => Err(UnknownTask {
                project: self.workspace,
                environment: self.name().clone(),
                platform,
                task_name: name.clone(),
            }),
            Ok(Some(task)) => Ok(task),
        }
    }

    /// Returns a map of all the features and their tasks for this environment.
    ///
    /// Resolves for the best platform target.
    pub fn feature_tasks(&self) -> HashMap<&'p FeatureName, HashMap<&'p TaskName, &'p Task>> {
        self.features()
            .map(|feature| {
                (
                    &feature.name,
                    feature
                        .targets
                        .resolve(Some(self.best_platform()))
                        .flat_map(|target| target.tasks.iter())
                        .collect::<HashMap<_, _>>(),
                )
            })
            .collect()
    }

    /// Returns the system requirements for this environment.
    ///
    /// The system requirements of the environment are the union of the system
    /// requirements of all the features that make up the environment. If
    /// multiple features specify a requirement for the same system package,
    /// the highest is chosen.
    ///
    /// If an environment defines a solve group the system requirements of all
    /// environments in the solve group are also combined. This means that
    /// if two environments in the same solve group specify conflicting
    /// system requirements that the highest system requirements are chosen.
    ///
    /// This is done to ensure that the requirements of all environments in the
    /// same solve group are compatible with each other.
    ///
    /// If you want to get the system requirements for this environment without
    /// taking the solve group into account, use the
    /// [`FeaturesExt::local_system_requirements`] method.
    pub fn system_requirements(&self) -> SystemRequirements {
        if let Some(solve_group) = self.solve_group() {
            solve_group.system_requirements()
        } else {
            self.local_system_requirements()
        }
    }

    /// Returns the activation scripts that should be run when activating this
    /// environment.
    ///
    /// The activation scripts of all features are combined in the order they
    /// are defined for the environment.
    pub(crate) fn activation_scripts(&self, platform: Option<Platform>) -> Vec<String> {
        self.features()
            .filter_map(|f| f.activation_scripts(platform))
            .flatten()
            .cloned()
            .collect()
    }

    /// Returns the environment variables that should be set when activating
    /// this environment.
    ///
    /// The environment variables of all features are combined in the order they
    /// are defined for the environment.
    pub(crate) fn activation_env(&self, platform: Option<Platform>) -> IndexMap<String, String> {
        // As self.features() would put "default" envs in the last item, but the "default" env priority should be the lowest.
        // Here, we use rfold (reverse fold) to ensure later features override earlier features
        // for environment variables. Processing features in reverse order means that
        // features appearing later in the list will have higher precedence.
        //
        // Example: If features: [override_feature, user_feature, default]
        // - rfold processes as: [default, user_feature, override_feature]
        // - Result: override_feature env vars take precedence over all others
        self.features().map(|f| f.activation_env(platform)).rfold(
            IndexMap::new(),
            |mut acc, env| {
                acc.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
                acc
            },
        )
    }

    /// Validates that the given platform is supported by this environment.
    fn validate_platform_support(
        &self,
        platform: Option<Platform>,
    ) -> Result<(), UnsupportedPlatformError> {
        if let Some(platform) = platform {
            if !self.platforms().contains(&platform) {
                return Err(UnsupportedPlatformError {
                    environments_platforms: self.platforms().into_iter().collect(),
                    environment: self.name().clone(),
                    platform,
                });
            }
        }

        Ok(())
    }

    /// Returns the channel configuration for this grouped environment
    pub fn channel_config(&self) -> ChannelConfig {
        self.workspace().channel_config()
    }

    // Extract variable name
    fn extract_variable_name(value: &str) -> Option<&str> {
        if let Some(inner) = value.strip_prefix('$') {
            Some(inner)
        } else if let Some(inner) = value.strip_prefix('%').and_then(|s| s.strip_suffix('%')) {
            Some(inner)
        } else if let Some(inner) = value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            Some(inner)
        } else {
            None
        }
    }

    /// Resolves variable references in environment variables
    pub fn resolve_variable_references<M>(map: &mut M)
    where
        M: Clone,
        for<'a> &'a M: std::iter::IntoIterator<Item = (&'a String, &'a String)>,
        M: std::iter::Extend<(String, String)>,
    {
        let map_clone = map.clone();
        let keys_to_update: Vec<String> =
            (&map_clone).into_iter().map(|(k, _)| k.clone()).collect();

        for key in keys_to_update {
            if let Some((_, value)) = (&map_clone).into_iter().find(|(k, _)| *k == &key) {
                if let Some(referenced_var) = Self::extract_variable_name(value) {
                    if let Some((_, actual_value)) = (&map_clone)
                        .into_iter()
                        .find(|(k, _)| k.as_str() == referenced_var)
                    {
                        map.extend([(key, actual_value.clone())]);
                    }
                }
            }
        }
    }
}

impl<'p> HasWorkspaceRef<'p> for Environment<'p> {
    fn workspace(&self) -> &'p Workspace {
        self.workspace
    }
}

impl<'p> HasWorkspaceManifest<'p> for Environment<'p> {
    fn workspace_manifest(&self) -> &'p WorkspaceManifest {
        self.workspace().workspace_manifest()
    }
}

impl<'p> HasFeaturesIter<'p> for Environment<'p> {
    /// Returns references to the features that make up this environment.
    fn features(&self) -> impl DoubleEndedIterator<Item = &'p Feature> + 'p {
        let manifest = self.workspace_manifest();
        let environment_features = self.environment.features.iter().map(|feature_name| {
            manifest
                .features
                .get(&FeatureName::from(feature_name.clone()))
                .expect("feature usage should have been validated upfront")
        });

        if self.environment.no_default_feature {
            Either::Right(environment_features)
        } else {
            Either::Left(environment_features.chain([self.workspace_manifest().default_feature()]))
        }
    }
}

impl Hash for Environment<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.environment.name.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, path::Path};

    use indexmap::indexmap;
    use insta::assert_snapshot;
    use itertools::Itertools;
    use pixi_manifest::CondaDependencies;

    use super::*;

    #[test]
    fn test_default_channels() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["foo", "bar"]
        platforms = []
        "#,
        )
        .unwrap();

        let channels = manifest
            .default_environment()
            .channels()
            .into_iter()
            .map(|c| c.as_str())
            .collect_vec();
        assert_eq!(channels, vec!["foo", "bar"]);
    }

    #[test]
    fn test_default_platforms() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = []
        platforms = ["linux-64", "osx-64"]
        "#,
        )
        .unwrap();

        let channels = manifest.default_environment().platforms();
        assert_eq!(
            channels,
            HashSet::from_iter([Platform::Linux64, Platform::Osx64,])
        );
    }

    #[test]
    fn test_default_tasks() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = []
        platforms = ["linux-64"]

        [tasks]
        foo = "echo default"

        [target.linux-64.tasks]
        foo = "echo linux"
        "#,
        )
        .unwrap();

        let task = manifest
            .default_environment()
            .task(&"foo".into(), None)
            .unwrap()
            .as_single_command(None)
            .unwrap()
            .unwrap();

        assert_eq!(task, "echo default");

        let task_osx = manifest
            .default_environment()
            .task(&"foo".into(), Some(Platform::Linux64))
            .unwrap()
            .as_single_command(None)
            .unwrap()
            .unwrap();

        assert_eq!(task_osx, "echo linux");

        assert!(
            manifest
                .default_environment()
                .tasks(Some(Platform::Osx64))
                .is_err()
        )
    }
    #[test]
    fn test_filtered_tasks() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = []
        platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]

        [tasks]
        foo = "echo foo"
        _bar = "echo bar"
        "#,
        )
        .unwrap();

        let task = manifest.default_environment().get_filtered_tasks();

        assert_eq!(task.len(), 1);
        assert!(task.contains(&"foo".into()));
    }

    fn format_dependencies(dependencies: CondaDependencies) -> String {
        dependencies
            .into_specs()
            .map(|(name, spec)| format!("{} = {}", name.as_source(), spec.to_toml_value()))
            .join("\n")
    }

    #[test]
    fn test_dependencies() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = []
        platforms = ["linux-64", "osx-64"]

        [dependencies]
        foo = "*"

        [build-dependencies]
        foo = "<4.0"

        [target.osx-64.dependencies]
        foo = "<5.0"

        [feature.foo.dependencies]
        foo = ">=1.0"

        [feature.bar.dependencies]
        bar = ">=1.0"
        foo = "<2.0"

        [environments]
        foobar = ["foo", "bar"]
        "#,
        )
        .unwrap();

        let deps = manifest
            .environment("foobar")
            .unwrap()
            .combined_dependencies(None);
        assert_snapshot!(format_dependencies(deps));
    }

    #[test]
    fn test_activation() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
            [project]
            name = "foobar"
            channels = []
            platforms = ["linux-64", "osx-64"]

            [activation]
            scripts = ["default.bat"]

            [target.linux-64.activation]
            scripts = ["linux.bat"]

            [feature.foo.activation]
            scripts = ["foo.bat"]

            [environments]
            foo = ["foo"]
            "#,
        )
        .unwrap();

        let foo_env = manifest.environment("foo").unwrap();
        assert_eq!(
            foo_env.activation_scripts(None),
            vec!["foo.bat".to_string(), "default.bat".to_string()]
        );
        assert_eq!(
            foo_env.activation_scripts(Some(Platform::Linux64)),
            vec!["foo.bat".to_string(), "linux.bat".to_string()]
        );
    }

    #[test]
    fn test_activation_env() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
            [project]
            name = "foobar"
            channels = []
            platforms = ["linux-64", "osx-64"]

            [activation.env]
            DEFAULT_VAR = "1"

            [target.linux-64.activation.env]
            LINUX_VAR = "1"

            [feature.foo.activation.env]
            FOO_VAR = "1"

            [environments]
            foo = ["foo"]
            "#,
        )
        .unwrap();

        let default_env = manifest.default_environment();
        let foo_env = manifest.environment("foo").unwrap();
        assert_eq!(
            foo_env.activation_env(None),
            indexmap! {
                "FOO_VAR".to_string() => "1".to_string(),
                "DEFAULT_VAR".to_string() => "1".to_string(),
            }
        );
        assert_eq!(
            default_env.activation_env(Some(Platform::Linux64)),
            indexmap! {
                "LINUX_VAR".to_string() => "1".to_string(),
                "DEFAULT_VAR".to_string() => "1".to_string(),
            }
        );
    }

    #[test]
    fn test_activation_env_priority() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
            [project]
            name = "foobar"
            channels = []
            platforms = ["linux-64", "osx-64"]

            [activation.env]
            FOO_VAR = "default"

            [feature.foo.activation.env]
            FOO_VAR = "foo"

            [feature.cuda1.activation.env]
            FOO_VAR = "cuda1"

            [feature.cuda2.activation.env]
            FOO_VAR = "cuda2"

            [environments]
            foo = ["foo"]
            cuda = ["cuda1", "cuda2"] 
            "#,
        )
        .unwrap();

        let default_env = manifest.default_environment();
        let foo_env = manifest.environment("foo").unwrap();
        let cuda_env = manifest.environment("cuda").unwrap();
        assert_eq!(
            default_env.activation_env(None),
            indexmap! {
                "FOO_VAR".to_string() => "default",
            }
        );
        assert_eq!(
            foo_env.activation_env(None),
            indexmap! {
                "FOO_VAR".to_string() => "foo",
            }
        );
        assert_eq!(
            cuda_env.activation_env(None),
            indexmap! {
                "FOO_VAR".to_string() => "cuda1",
            }
        );
    }

    #[test]
    fn test_channel_feature_priority() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["a", "b"]
        platforms = ["linux-64", "osx-64"]

        [feature.foo]
        channels = ["c", "d"]

        [feature.bar]
        channels = ["e", "f"]

        [feature.barfoo]
        channels = ["a", "f"]

        [environments]
        foo = ["foo"]
        foobar = ["foo", "bar"]
        barfoo = {features = ["barfoo"], no-default-feature=true}
        "#,
        )
        .unwrap();

        // All channels are added in order of the features and default is last
        let foobar_channels = manifest.environment("foobar").unwrap().channels();
        assert_eq!(
            foobar_channels
                .into_iter()
                .map(|c| c.to_string())
                .collect_vec(),
            vec!["c", "d", "e", "f", "a", "b"]
        );

        let foo_channels = manifest.environment("foo").unwrap().channels();
        assert_eq!(
            foo_channels
                .into_iter()
                .map(|c| c.to_string())
                .collect_vec(),
            vec!["c", "d", "a", "b"]
        );

        // The default feature is not included in the channels, so only the feature
        // channels are included.
        let barfoo_channels = manifest.environment("barfoo").unwrap().channels();
        assert_eq!(
            barfoo_channels
                .into_iter()
                .map(|c| c.to_string())
                .collect_vec(),
            vec!["a", "f"]
        )
    }

    #[test]
    fn test_channel_feature_priority_with_redefinition() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "test"
        channels = ["d", "a", "b"]
        platforms = ["linux-64"]

        [environments]
        foo = ["foo"]

        [feature.foo]
        channels = ["a", "c", "b"]

        "#,
        )
        .unwrap();

        let foobar_channels = manifest.environment("default").unwrap().channels();
        assert_eq!(
            foobar_channels
                .into_iter()
                .map(|c| c.to_string())
                .collect_vec(),
            vec!["d", "a", "b"]
        );

        // Check if the feature channels are sorted correctly,
        // and that the remaining channels from the default feature are appended.
        let foo_channels = manifest.environment("foo").unwrap().channels();
        assert_eq!(
            foo_channels
                .into_iter()
                .map(|c| c.to_string())
                .collect_vec(),
            vec!["a", "c", "b", "d"]
        );
    }

    #[test]
    fn test_channel_priorities() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64"]

        [feature.foo]
        channels = [{channel = "nvidia", priority = 1}, "pytorch"]

        [feature.bar]
        channels = [{ channel = "bar", priority = -10 }, "barry"]

        [environments]
        foo = ["foo"]
        bar = ["bar"]
        foobar = ["foo", "bar"]
        "#,
        )
        .unwrap();

        let foobar_channels = manifest.environment("foobar").unwrap().channels();
        assert_eq!(
            foobar_channels
                .into_iter()
                .map(|c| c.to_string())
                .collect_vec(),
            vec!["nvidia", "pytorch", "barry", "conda-forge", "bar"]
        );
        let foo_channels = manifest.environment("foo").unwrap().channels();
        assert_eq!(
            foo_channels
                .into_iter()
                .map(|c| c.to_string())
                .collect_vec(),
            vec!["nvidia", "pytorch", "conda-forge"]
        );

        let bar_channels = manifest.environment("bar").unwrap().channels();
        assert_eq!(
            bar_channels
                .into_iter()
                .map(|c| c.to_string())
                .collect_vec(),
            vec!["barry", "conda-forge", "bar"]
        );
    }

    #[test]
    fn test_pypi_options_per_environment() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64"]

        [feature.foo]
        pypi-options = { index-url = "https://mypypi.org/simple", extra-index-urls = ["https://1.com"] }

        [feature.bar]
        pypi-options = { extra-index-urls = ["https://2.com"] }

        [environments]
        foo = ["foo"]
        bar = ["bar"]
        foobar = ["foo", "bar"]
        "#,
        )
        .unwrap();

        let foo_opts = manifest.environment("foo").unwrap().pypi_options();
        assert_eq!(
            foo_opts.index_url.unwrap().to_string(),
            "https://mypypi.org/simple"
        );
        assert_eq!(
            foo_opts
                .extra_index_urls
                .unwrap()
                .iter()
                .map(|i| i.to_string())
                .collect_vec(),
            vec!["https://1.com/"]
        );

        let bar_opts = manifest.environment("bar").unwrap().pypi_options();
        assert_eq!(
            bar_opts
                .extra_index_urls
                .unwrap()
                .iter()
                .map(|i| i.to_string())
                .collect_vec(),
            vec!["https://2.com/"]
        );

        let foo_bar_opts = manifest.environment("foobar").unwrap().pypi_options();

        assert_eq!(
            foo_bar_opts.index_url.unwrap().to_string(),
            "https://mypypi.org/simple"
        );

        assert_eq!(
            foo_bar_opts
                .extra_index_urls
                .unwrap()
                .iter()
                .map(|i| i.to_string())
                .collect_vec(),
            vec!["https://1.com/", "https://2.com/"]
        )
    }

    #[test]
    fn test_pypi_options_project_and_default_feature() {
        let contents = r##"
            [project]
            name = "foobar"
            channels = ["conda-forge"]
            platforms = ["osx-64", "linux-64", "win-64"]

            [project.pypi-options]
            extra-index-urls = ["https://pypi.org/simple2"]

            # These are added to the default feature
            [feature.foo.pypi-options]
            extra-index-urls = ["https://pypi.org/simple"]

            [environments]
            foo = ["foo"]
            bar = { features = ["foo"], no-default-feature = true }
            "##;

        let manifest = Workspace::from_str(Path::new("pixi.toml"), contents).unwrap();
        assert_eq!(
            manifest
                .default_environment()
                .pypi_options()
                .extra_index_urls
                .unwrap()
                .len(),
            1
        );
        let foo_opts = manifest.environment("foo").unwrap().pypi_options();
        let bar_opts = manifest.environment("bar").unwrap().pypi_options();
        // Includes default pypl options, inherited from project
        // and the one from the feature
        assert_eq!(foo_opts.extra_index_urls.unwrap().len(), 2);
        assert_eq!(bar_opts.extra_index_urls.unwrap().len(), 1);
    }

    #[test]
    fn test_validate_platform() {
        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["conda-forge"]
        platforms = ["osx-64", "linux-64", "win-64"]
        "#,
        )
        .unwrap();
        let env = manifest.default_environment();
        // This should also work on OsxArm64
        assert!(env.validate_platform_support(Some(Platform::Osx64)).is_ok());

        let manifest = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["conda-forge"]
        platforms = ["emscripten-wasm32"]
        "#,
        )
        .unwrap();
        let env = manifest.default_environment();
        assert!(
            env.validate_platform_support(Some(Platform::EmscriptenWasm32))
                .is_ok()
        );
    }
}
