use crate::activation::Activation;
use crate::environment::{Environment, EnvironmentIdx, EnvironmentName, TomlEnvironmentMapOrSeq};
use crate::environments::Environments;
use crate::error::TomlError;
use crate::feature::{Feature, FeatureName};
use crate::metadata::ProjectMetadata;
use crate::pypi::pypi_options::PypiOptions;
use crate::pypi::pypi_requirement::PyPiRequirement;
use crate::pypi::pypi_requirement_types::PyPiPackageName;
use crate::solve_group::SolveGroups;
use crate::spec_type::SpecType;
use crate::system_requirements::SystemRequirements;
use crate::target::{Target, TargetSelector, Targets};
use crate::task::{Task, TaskName};
use crate::utils::PixiSpanned;
use crate::{consts, nameless_matchspec::NamelessMatchSpecWrapper};
use indexmap::map::IndexMap;
use indexmap::Equivalent;
use rattler_conda_types::NamelessMatchSpec;
use rattler_conda_types::PackageName;
use serde::de::{Deserialize, DeserializeSeed, Deserializer, MapAccess, Visitor};
use serde_with::serde_as;
use serde_with::serde_derive::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::iter::FromIterator;
use std::marker::PhantomData;
use toml_edit::DocumentMut;

/// Describes the contents of a parsed project manifest.
#[derive(Debug, Clone)]
pub struct ParsedManifest {
    /// Information about the project
    pub project: ProjectMetadata,

    /// All the features defined in the project.
    pub features: IndexMap<FeatureName, Feature>,

    /// All the environments defined in the project.
    pub environments: Environments,

    /// The solve groups that are part of the project.
    pub solve_groups: SolveGroups,
}

impl ParsedManifest {
    /// Parses a toml string into a project manifest.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        let manifest: ParsedManifest = toml_edit::de::from_str(source).map_err(TomlError::from)?;

        // Make sure project.name is defined
        if manifest.project.name.is_none() {
            let span = source.parse::<DocumentMut>().map_err(TomlError::from)?["project"].span();
            return Err(TomlError::NoProjectName(span));
        }

        Ok(manifest)
    }

    /// Set the project name.
    pub(crate) fn set_project_name(&mut self, name: String) {
        self.project.name = Some(name);
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
    pub fn environment<Q: ?Sized>(&self, name: &Q) -> Option<&Environment>
    where
        Q: Hash + Equivalent<EnvironmentName>,
    {
        self.environments.find(name)
    }
}

impl<'de> Deserialize<'de> for ParsedManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        pub struct TomlProjectManifest {
            project: ProjectMetadata,
            #[serde(default)]
            system_requirements: SystemRequirements,
            #[serde(default)]
            target: IndexMap<PixiSpanned<TargetSelector>, Target>,

            // HACK: If we use `flatten`, unknown keys will point to the wrong location in the
            // file.  When https://github.com/toml-rs/toml/issues/589 is fixed we should use that
            //
            // Instead we currently copy the keys from the Target deserialize implementation which
            // is really ugly.
            //
            // #[serde(flatten)]
            // default_target: Target,
            #[serde(default, deserialize_with = "deserialize_package_map")]
            dependencies: IndexMap<PackageName, NamelessMatchSpec>,

            #[serde(default, deserialize_with = "deserialize_opt_package_map")]
            host_dependencies: Option<IndexMap<PackageName, NamelessMatchSpec>>,

            #[serde(default, deserialize_with = "deserialize_opt_package_map")]
            build_dependencies: Option<IndexMap<PackageName, NamelessMatchSpec>>,

            #[serde(default)]
            pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

            /// Additional information to activate an environment.
            #[serde(default)]
            activation: Option<Activation>,

            /// Target specific tasks to run in the environment
            #[serde(default)]
            tasks: HashMap<TaskName, Task>,

            /// The features defined in the project.
            #[serde(default)]
            feature: IndexMap<FeatureName, Feature>,

            /// The environments the project can create.
            #[serde(default)]
            environments: IndexMap<EnvironmentName, TomlEnvironmentMapOrSeq>,

            /// pypi-options
            #[serde(default)]
            pypi_options: Option<PypiOptions>,

            /// The tool configuration which is unused by pixi
            #[serde(default, skip_serializing, rename = "tool")]
            _tool: serde::de::IgnoredAny,

            /// The URI for the manifest schema which is unused by pixi
            #[allow(dead_code)]
            #[serde(rename = "$schema")]
            schema: Option<String>,
        }

        let toml_manifest = TomlProjectManifest::deserialize(deserializer)?;
        let mut dependencies = HashMap::from_iter([(SpecType::Run, toml_manifest.dependencies)]);
        if let Some(host_deps) = toml_manifest.host_dependencies {
            dependencies.insert(SpecType::Host, host_deps);
        }
        if let Some(build_deps) = toml_manifest.build_dependencies {
            dependencies.insert(SpecType::Build, build_deps);
        }

        let default_target = Target {
            dependencies,
            pypi_dependencies: toml_manifest.pypi_dependencies,
            activation: toml_manifest.activation,
            tasks: toml_manifest.tasks,
        };

        // Construct a default feature
        let default_feature = Feature {
            name: FeatureName::Default,

            // The default feature does not overwrite the platforms or channels from the project
            // metadata.
            platforms: None,
            channels: None,

            channel_priority: toml_manifest.project.channel_priority,

            system_requirements: toml_manifest.system_requirements,

            // Use the pypi-options from the manifest for
            // the default feature
            pypi_options: toml_manifest.pypi_options,

            // Combine the default target with all user specified targets
            targets: Targets::from_default_and_user_defined(default_target, toml_manifest.target),
        };

        // Construct the features including the default feature
        let features: IndexMap<FeatureName, Feature> =
            IndexMap::from_iter([(FeatureName::Default, default_feature)]);
        let named_features = toml_manifest
            .feature
            .into_iter()
            .map(|(name, mut feature)| {
                feature.name = name.clone();
                (name, feature)
            })
            .collect::<IndexMap<FeatureName, Feature>>();
        let features = features.into_iter().chain(named_features).collect();

        // Construct the environments including the default environment
        let mut environments = Environments::default();
        let mut solve_groups = SolveGroups::default();

        // Add the default environment first if it was not redefined.
        if !toml_manifest
            .environments
            .contains_key(&EnvironmentName::Default)
        {
            environments.environments.push(Some(Environment::default()));
            environments
                .by_name
                .insert(EnvironmentName::Default, EnvironmentIdx(0));
        }

        // Add all named environments
        for (name, env) in toml_manifest.environments {
            // Decompose the TOML
            let (features, features_source_loc, solve_group, no_default_feature) = match env {
                TomlEnvironmentMapOrSeq::Map(env) => (
                    env.features.value,
                    env.features.span,
                    env.solve_group,
                    env.no_default_feature,
                ),
                TomlEnvironmentMapOrSeq::Seq(features) => (features, None, None, false),
            };

            let environment_idx = EnvironmentIdx(environments.environments.len());
            environments.by_name.insert(name.clone(), environment_idx);
            environments.environments.push(Some(Environment {
                name,
                features,
                features_source_loc,
                solve_group: solve_group.map(|sg| solve_groups.add(sg, environment_idx)),
                no_default_feature,
            }));
        }

        Ok(Self {
            project: toml_manifest.project,
            features,
            environments,
            solve_groups,
        })
    }
}

struct PackageMap<'a>(&'a IndexMap<PackageName, NamelessMatchSpec>);

impl<'de, 'a> DeserializeSeed<'de> for PackageMap<'a> {
    type Value = PackageName;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let package_name = PackageName::deserialize(deserializer)?;
        match self.0.get_key_value(&package_name) {
            Some((package_name, _)) => {
                Err(serde::de::Error::custom(
                    format!(
                        "duplicate dependency: {} (please avoid using capitalized names for the dependencies)", package_name.as_source())
                ))
            }
            None => Ok(package_name),
        }
    }
}

pub fn deserialize_package_map<'de, D>(
    deserializer: D,
) -> Result<IndexMap<PackageName, NamelessMatchSpec>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PackageMapVisitor(PhantomData<()>);

    impl<'de> Visitor<'de> for PackageMapVisitor {
        type Value = IndexMap<PackageName, NamelessMatchSpec>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a map")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut result = IndexMap::new();
            let match_spec = NamelessMatchSpecWrapper {};
            while let Some((package_name, match_spec)) = map
                .next_entry_seed::<PackageMap, &NamelessMatchSpecWrapper>(
                    PackageMap(&result),
                    &match_spec,
                )?
            {
                result.insert(package_name, match_spec);
            }

            Ok(result)
        }
    }
    let visitor = PackageMapVisitor(PhantomData);
    deserializer.deserialize_seq(visitor)
}

pub fn deserialize_opt_package_map<'de, D>(
    deserializer: D,
) -> Result<Option<IndexMap<PackageName, NamelessMatchSpec>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(deserialize_package_map(deserializer)?))
}

#[cfg(test)]
mod tests {
    use crate::parsed_manifest::ParsedManifest;
    use crate::TargetSelector;
    use insta::{assert_snapshot, assert_yaml_snapshot};
    use itertools::Itertools;
    use rattler_conda_types::{Channel, Platform};

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

        let manifest = ParsedManifest::from_toml_str(&contents).unwrap();
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
                .to_string(),
            "==3.4.5"
        );
        assert_eq!(
            osx64_target
                .run_dependencies()
                .unwrap()
                .get("foo")
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

        let manifest = ParsedManifest::from_toml_str(&contents).unwrap();
        let deps = manifest
            .default_feature()
            .targets
            .default()
            .run_dependencies()
            .unwrap();
        let test_map_spec = deps.get("test_map").unwrap();

        assert_eq!(test_map_spec.to_string(), ">=1.2.3 py34_0");
        assert_eq!(
            test_map_spec
                .channel
                .as_deref()
                .map(Channel::canonical_name),
            Some(String::from("https://conda.anaconda.org/conda-forge/"))
        );

        assert_eq!(deps.get("test_build").unwrap().to_string(), "* bla");

        let test_channel = deps.get("test_channel").unwrap();
        assert_eq!(test_channel.to_string(), "*");
        assert_eq!(
            test_channel.channel.as_deref().map(Channel::canonical_name),
            Some(String::from("https://conda.anaconda.org/conda-forge/"))
        );

        let test_version = deps.get("test_version").unwrap();
        assert_eq!(test_version.to_string(), ">=1.2.3");

        let test_version_channel = deps.get("test_version_channel").unwrap();
        assert_eq!(test_version_channel.to_string(), ">=1.2.3");
        assert_eq!(
            test_version_channel
                .channel
                .as_deref()
                .map(Channel::canonical_name),
            Some(String::from("https://conda.anaconda.org/conda-forge/"))
        );

        let test_version_build = deps.get("test_version_build").unwrap();
        assert_eq!(test_version_build.to_string(), ">=1.2.3 py34_0");
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

        let manifest = ParsedManifest::from_toml_str(&contents).unwrap();
        let default_target = manifest.default_feature().targets.default();
        let run_dependencies = default_target.run_dependencies().unwrap();
        let build_dependencies = default_target.build_dependencies().unwrap();
        let host_dependencies = default_target.host_dependencies().unwrap();

        assert_eq!(
            run_dependencies.get("my-game").unwrap().to_string(),
            "==1.0.0"
        );
        assert_eq!(build_dependencies.get("cmake").unwrap().to_string(), "*");
        assert_eq!(host_dependencies.get("sdl2").unwrap().to_string(), "*");
    }

    #[test]
    fn test_invalid_target_specific() {
        let examples = [r#"[target.foobar.dependencies]
            invalid_platform = "henk""#];

        assert_snapshot!(examples
            .into_iter()
            .map(|example| ParsedManifest::from_toml_str(&format!(
                "{PROJECT_BOILERPLATE}\n{example}"
            ))
            .unwrap_err()
            .to_string())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[test]
    fn test_invalid_key() {
        let examples = [
            format!("{PROJECT_BOILERPLATE}\n[foobar]"),
            format!("{PROJECT_BOILERPLATE}\n[target.win-64.hostdependencies]"),
        ];
        assert_snapshot!(examples
            .into_iter()
            .map(|example| ParsedManifest::from_toml_str(&example)
                .unwrap_err()
                .to_string())
            .collect::<Vec<_>>()
            .join("\n"))
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

        let manifest = ParsedManifest::from_toml_str(&contents).unwrap();

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

        assert_snapshot!(toml_edit::de::from_str::<ParsedManifest>(&contents)
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

        assert_yaml_snapshot!(toml_edit::de::from_str::<ParsedManifest>(&contents)
            .expect("parsing should succeed!")
            .project
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
            toml_edit::de::from_str::<ParsedManifest>(&contents).expect("parsing should succeed!");
        assert_yaml_snapshot!(manifest.project.pypi_options.clone().unwrap());
    }

    #[test]
    fn test_duplicate_dependency() {
        let contents = format!(
            r#"
        {PROJECT_BOILERPLATE}

        [dependencies]
        Flask = "2.*"
        flask = "2.*"
        "#
        );
        let manifest = ParsedManifest::from_toml_str(&contents);

        assert!(manifest.is_err());
        assert!(manifest
            .unwrap_err()
            .to_string()
            .contains("duplicate dependency"));
    }

    #[test]
    fn test_duplicate_host_dependency() {
        let contents = format!(
            r#"
        {PROJECT_BOILERPLATE}

        [host-dependencies]
        LibC = "2.12"
        libc = "2.12"
        "#
        );
        let manifest = ParsedManifest::from_toml_str(&contents);

        assert!(manifest.is_err());
        assert!(manifest
            .unwrap_err()
            .to_string()
            .contains("duplicate dependency"));
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
        let _manifest = ParsedManifest::from_toml_str(contents).unwrap();
    }
}
