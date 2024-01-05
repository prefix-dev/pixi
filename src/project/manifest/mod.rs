mod environment;
mod error;
mod feature;
mod target;

use crate::{
    consts,
    project::{manifest::target::Targets, python::PyPiRequirement, DependencyType, SpecType},
    task::Task,
    utils::spanned::PixiSpanned,
};
use ::serde::Deserialize;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, NamelessMatchSpec, PackageName, Platform, Version,
};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use serde::Deserializer;
use serde_with::{serde_as, DisplayFromStr};
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    str::FromStr,
};
use toml_edit::{value, Array, Document, Item, Table, TomlError, Value};
use url::Url;

pub use environment::{Environment, EnvironmentName};
pub use feature::{Feature, FeatureName};
pub use target::{Target, TargetSelector};

/// Handles the project's manifest file.
/// This struct is responsible for reading, parsing, editing, and saving the manifest.
/// It encapsulates all logic related to the manifest's TOML format and structure.
/// The manifest data is represented as a [`ProjectManifest`] struct for easy manipulation.
/// Owned by the [`crate::project::Project`] struct, which governs its usage.
///
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The path to the manifest file
    pub path: PathBuf,

    /// The raw contents of the manifest file
    pub contents: String,

    /// Editable toml document
    pub document: toml_edit::Document,

    /// The parsed manifest
    pub parsed: ProjectManifest,
}

impl Manifest {
    /// Create a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let contents = std::fs::read_to_string(path.as_ref()).into_diagnostic()?;
        let parent = path
            .as_ref()
            .parent()
            .expect("Path should always have a parent");
        Self::from_str(parent, contents)
    }

    /// Create a new manifest from a string
    pub fn from_str(root: &Path, contents: impl Into<String>) -> miette::Result<Self> {
        let contents = contents.into();
        let (manifest, document) = match ProjectManifest::from_toml_str(&contents)
            .and_then(|manifest| contents.parse::<Document>().map(|doc| (manifest, doc)))
        {
            Ok(result) => result,
            Err(e) => {
                if let Some(span) = e.span() {
                    return Err(miette::miette!(
                        labels = vec![LabeledSpan::at(span, e.message())],
                        "failed to parse project manifest"
                    )
                    .with_source_code(NamedSource::new(consts::PROJECT_MANIFEST, contents)));
                } else {
                    return Err(e).into_diagnostic();
                }
            }
        };

        // Validate the contents of the manifest
        manifest.validate(
            NamedSource::new(consts::PROJECT_MANIFEST, contents.to_owned()),
            root,
        )?;

        // Notify the user that pypi-dependencies are still experimental
        if manifest
            .features
            .values()
            .flat_map(|f| f.targets.targets())
            .any(|f| f.pypi_dependencies.is_some())
        {
            match std::env::var("PIXI_BETA_WARNING_OFF") {
                Ok(var) if var == *"true" => {}
                _ => {
                    tracing::warn!("BETA feature `[pypi-dependencies]` enabled!\n\nPlease report any and all issues here:\n\n\thttps://github.com/prefix-dev/pixi.\n\nTurn this warning off by setting the environment variable `PIXI_BETA_WARNING_OFF` to `true`.\n");
                }
            }
        }

        Ok(Self {
            path: root.join(consts::PROJECT_MANIFEST),
            contents,
            document,
            parsed: manifest,
        })
    }

    /// Save the manifest to the file and update the contents
    pub fn save(&mut self) -> miette::Result<()> {
        self.contents = self.document.to_string();
        std::fs::write(&self.path, self.contents.clone()).into_diagnostic()?;
        Ok(())
    }

    /// Returns a hashmap of the tasks that should run only the given platform. If the platform is
    /// `None`, only the default targets tasks are returned.
    pub fn tasks(&self, platform: Option<Platform>) -> HashMap<&str, &Task> {
        self.default_feature()
            .targets
            .resolve(platform)
            .collect_vec()
            .into_iter()
            .rev()
            .flat_map(|target| target.tasks.iter())
            .map(|(name, task)| (name.as_str(), task))
            .collect()
    }

    /// Add a task to the project
    pub fn add_task(
        &mut self,
        name: impl AsRef<str>,
        task: Task,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        // Check if the task already exists
        if self.tasks(platform).contains_key(name.as_ref()) {
            miette::bail!("task {} already exists", name.as_ref());
        }

        // Get the table that contains the tasks.
        let table = ensure_toml_target_table(&mut self.document, platform, "tasks")?;

        // Add the task to the table
        table.insert(name.as_ref(), task.clone().into());

        // Add the task to the manifest
        self.default_feature_mut()
            .targets
            .for_opt_target_mut_or_default(platform.map(TargetSelector::from).as_ref())
            .tasks
            .insert(name.as_ref().to_string(), task);

        Ok(())
    }

    /// Remove a task from the project, and the tasks that depend on it
    pub fn remove_task(
        &mut self,
        name: impl AsRef<str>,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        self.tasks(platform)
            .get(name.as_ref())
            .ok_or_else(|| miette::miette!("task {} does not exist", name.as_ref()))?;

        // Get the task table either from the target platform or the default tasks.
        let tasks_table = ensure_toml_target_table(&mut self.document, platform, "tasks")?;

        // If it does not exist in toml, consider this ok as we want to remove it anyways
        tasks_table.remove(name.as_ref());

        // Remove the task from the internal manifest
        self.default_feature_mut()
            .targets
            .for_opt_target_mut(platform.map(TargetSelector::from).as_ref())
            .map(|target| target.tasks.remove(name.as_ref()));

        Ok(())
    }

    /// Add a platform to the project
    pub fn add_platforms<'a>(
        &mut self,
        platforms: impl Iterator<Item = &'a Platform> + Clone,
    ) -> miette::Result<()> {
        // Add to platform table
        let platform_array = &mut self.document["project"]["platforms"];
        let platform_array = platform_array
            .as_array_mut()
            .expect("platforms should be an array");

        for platform in platforms.clone() {
            platform_array.push(platform.to_string());
        }

        // Add to manifest
        self.parsed.project.platforms.value.extend(platforms);
        Ok(())
    }

    /// Remove the platform(s) from the project
    pub fn remove_platforms(
        &mut self,
        platforms: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> miette::Result<()> {
        let mut removed_platforms = Vec::new();

        for platform in platforms {
            // Parse the channel to be removed
            let platform_to_remove = Platform::from_str(platform.as_ref()).into_diagnostic()?;

            // Remove the channel if it exists
            if let Some(pos) = self
                .parsed
                .project
                .platforms
                .value
                .iter()
                .position(|x| *x == platform_to_remove)
            {
                self.parsed.project.platforms.value.remove(pos);
            }

            removed_platforms.push(platform.as_ref().to_owned());
        }

        // remove the platforms from the toml
        let platform_array = &mut self.document["project"]["platforms"];
        let platform_array = platform_array
            .as_array_mut()
            .expect("platforms should be an array");

        platform_array.retain(|x| !removed_platforms.contains(&x.as_str().unwrap().to_string()));

        Ok(())
    }

    /// Add a matchspec to the manifest
    pub fn add_dependency(
        &mut self,
        spec: &MatchSpec,
        spec_type: SpecType,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        // Find the table toml table to add the dependency to.
        let dependency_table =
            ensure_toml_target_table(&mut self.document, platform, spec_type.name())?;

        // Determine the name of the package to add
        let (Some(name), spec) = spec.clone().into_nameless() else {
            miette::bail!("pixi does not support wildcard dependencies")
        };

        // Store (or replace) in the document
        dependency_table.insert(name.as_source(), Item::Value(spec.to_string().into()));

        // Add the dependency to the manifest as well
        self.default_feature_mut()
            .targets
            .for_opt_target_mut_or_default(platform.map(TargetSelector::from).as_ref())
            .dependencies
            .entry(spec_type)
            .or_default()
            .insert(name, spec);

        Ok(())
    }

    pub fn add_pypi_dependency(
        &mut self,
        name: &rip::types::PackageName,
        requirement: &PyPiRequirement,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        // Find the table toml table to add the dependency to.
        let dependency_table = ensure_toml_target_table(
            &mut self.document,
            platform,
            DependencyType::PypiDependency.name(),
        )?;

        // Add the pypi dependency to the table
        dependency_table.insert(name.as_str(), (*requirement).clone().into());

        // Add the dependency to the manifest as well
        self.default_feature_mut()
            .targets
            .for_opt_target_mut_or_default(platform.map(TargetSelector::from).as_ref())
            .pypi_dependencies
            .get_or_insert_with(Default::default)
            .insert(name.clone(), requirement.clone());

        Ok(())
    }

    /// Removes a dependency from `pixi.toml` based on `SpecType`.
    pub fn remove_dependency(
        &mut self,
        dep: &PackageName,
        spec_type: SpecType,
        platform: Option<Platform>,
    ) -> miette::Result<(PackageName, NamelessMatchSpec)> {
        get_toml_target_table(&mut self.document, platform, spec_type.name())?
            .remove(dep.as_normalized())
            .ok_or_else(|| {
                miette::miette!(
                    "Couldn't find {} in [{}]",
                    console::style(dep.as_normalized()).bold(),
                    console::style(spec_type.name()).bold(),
                )
            })?;

        Ok(self
            .default_feature_mut()
            .targets
            .for_opt_target_mut(platform.map(TargetSelector::Platform).as_ref())
            .expect("target should exist")
            .remove_dependency(dep.as_source(), spec_type)
            .expect("dependency should exist"))
    }

    /// Returns true if any of the features has pypi dependencies defined.
    ///
    /// This also returns true if the `pypi-dependencies` key is defined but empty.
    pub fn has_pypi_dependencies(&self) -> bool {
        self.parsed
            .features
            .values()
            .flat_map(|f| f.targets.targets())
            .any(|f| f.pypi_dependencies.is_some())
    }

    /// Returns a mutable reference to the channels array.
    fn channels_array_mut(&mut self) -> miette::Result<&mut Array> {
        let project = &mut self.document["project"];
        if project.is_none() {
            *project = Item::Table(Table::new());
        }

        let channels = &mut project["channels"];
        if channels.is_none() {
            *channels = Item::Value(Value::Array(Array::new()))
        }

        channels
            .as_array_mut()
            .ok_or_else(|| miette::miette!("malformed channels array"))
    }

    /// Adds the specified channels to the manifest.
    pub fn add_channels(
        &mut self,
        channels: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> miette::Result<()> {
        let mut stored_channels = Vec::new();
        for channel in channels {
            self.parsed.project.channels.push(
                Channel::from_str(channel.as_ref(), &ChannelConfig::default()).into_diagnostic()?,
            );
            stored_channels.push(channel.as_ref().to_owned());
        }

        let channels_array = self.channels_array_mut()?;
        for channel in stored_channels {
            channels_array.push(channel);
        }

        Ok(())
    }

    /// Remove the specified channels to the manifest.
    pub fn remove_channels(
        &mut self,
        channels: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> miette::Result<()> {
        let mut removed_channels = Vec::new();

        for channel in channels {
            // Parse the channel to be removed
            let channel_to_remove =
                Channel::from_str(channel.as_ref(), &ChannelConfig::default()).into_diagnostic()?;

            // Remove the channel if it exists
            if let Some(pos) = self
                .parsed
                .project
                .channels
                .iter()
                .position(|x| *x == channel_to_remove)
            {
                self.parsed.project.channels.remove(pos);
            }

            removed_channels.push(channel.as_ref().to_owned());
        }

        // remove the channels from the toml
        let channels_array = self.channels_array_mut()?;
        channels_array.retain(|x| !removed_channels.contains(&x.as_str().unwrap().to_string()));

        Ok(())
    }

    /// Set the project description
    pub fn set_description(&mut self, description: &String) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.parsed.project.description = Some(description.to_string());
        self.document["project"]["description"] = value(description);

        Ok(())
    }

    /// Set the project version
    pub fn set_version(&mut self, version: &String) -> miette::Result<()> {
        // Update in both the manifest and the toml
        self.parsed.project.version = Some(Version::from_str(version).unwrap());
        self.document["project"]["version"] = value(version);

        Ok(())
    }

    /// Returns the default feature.
    ///
    /// This is the feature that is added implicitly by the tables at the root of the project
    /// manifest.
    pub fn default_feature(&self) -> &Feature {
        self.parsed.default_feature()
    }

    /// Returns a mutable reference to the default feature.
    fn default_feature_mut(&mut self) -> &mut Feature {
        self.parsed.default_feature_mut()
    }

    /// Returns the default environment
    ///
    /// This is the environment that is added implicitly as the environment with only the default
    /// feature. The default environment can be overwritten by a environment named `default`.
    pub fn default_environment(&self) -> &Environment {
        self.parsed.default_environment()
    }
}

/// Ensures that the specified TOML target table exists within a given document,
/// and inserts it if not.
/// Returns the final target table (`table_name`) inside the platform-specific table if everything
/// goes as expected.
pub fn ensure_toml_target_table<'a>(
    doc: &'a mut Document,
    platform: Option<Platform>,
    table_name: &str,
) -> miette::Result<&'a mut Table> {
    let root_table = if let Some(platform) = platform {
        // Get or create the target table (e.g. [target])
        let target = doc["target"]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!("target table in {} is malformed", consts::PROJECT_MANIFEST)
            })?;
        target.set_dotted(true);

        // Add a specific platform table (e.g. [target.linux-64])
        let platform_table = doc["target"][platform.as_str()]
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                miette::miette!(
                    "platform table in {} is malformed",
                    consts::PROJECT_MANIFEST
                )
            })?;
        platform_table.set_dotted(true);
        platform_table
    } else {
        doc.as_table_mut()
    };

    // Return final table on target platform table.
    root_table[table_name]
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            miette::miette!(
                "{table_name} table in {} is malformed",
                consts::PROJECT_MANIFEST,
            )
        })
}

/// Retrieve a mutable reference to a target table `table_name`
/// for a specific platform.
fn get_toml_target_table<'a>(
    doc: &'a mut Document,
    platform: Option<Platform>,
    table_name: &str,
) -> miette::Result<&'a mut Table> {
    let target_table = if let Some(platform) = platform {
        doc["target"][platform.as_str()]
            .as_table_mut()
            .ok_or(miette::miette!(
                "could not find {} in {}",
                console::style(platform.as_str()).bold(),
                consts::PROJECT_MANIFEST,
            ))?
    } else {
        doc.as_table_mut()
    };

    target_table[table_name].as_table_mut().ok_or_else(|| {
        let table_name = match platform {
            Some(platform) => format!("target.{}.{}", platform.as_str(), table_name),
            None => table_name.to_string(),
        };
        miette::miette!(
            "could not find {} in {}",
            console::style(format!("[{table_name}]")).bold(),
            consts::PROJECT_MANIFEST,
        )
    })
}

/// Describes the contents of a project manifest.
#[derive(Debug, Clone)]
pub struct ProjectManifest {
    /// Information about the project
    pub project: ProjectMetadata,

    /// All the features defined in the project.
    pub features: IndexMap<FeatureName, Feature>,

    /// All the environments defined in the project.
    pub environments: IndexMap<EnvironmentName, Environment>,
}

impl ProjectManifest {
    /// Parses a toml string into a project manifest.
    pub fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_edit::de::from_str(source).map_err(TomlError::from)
    }

    /// Returns the default feature.
    ///
    /// This is the feature that is added implicitly by the tables at the root of the project
    /// manifest.
    pub fn default_feature(&self) -> &Feature {
        self.features
            .get(&FeatureName::Default)
            .expect("default feature should always exist")
    }

    /// Returns a mutable reference to the default feature.
    fn default_feature_mut(&mut self) -> &mut Feature {
        self.features
            .get_mut(&FeatureName::Default)
            .expect("default feature should always exist")
    }

    /// Returns the default environment
    ///
    /// This is the environment that is added implicitly as the environment with only the default
    /// feature. The default environment can be overwritten by a environment named `default`.
    pub fn default_environment(&self) -> &Environment {
        let envs = &self.environments;
        envs.get(&EnvironmentName::Named(String::from("default")))
            .or_else(|| envs.get(&EnvironmentName::Default))
            .expect("default environment should always exist")
    }
}

impl<'de> Deserialize<'de> for ProjectManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        pub struct TomlProjectManifest {
            project: ProjectMetadata,
            #[serde(default)]
            system_requirements: SystemRequirements,
            #[serde(default)]
            target: IndexMap<PixiSpanned<TargetSelector>, Target>,
            #[serde(flatten)]
            default_target: Target,
        }

        let toml_manifest = TomlProjectManifest::deserialize(deserializer)?;

        // Construct a default feature
        let default_feature = Feature {
            name: FeatureName::Default,

            // The default feature does not overwrite the platforms or channels from the project
            // metadata.
            platforms: None,
            channels: None,

            system_requirements: toml_manifest.system_requirements,

            // Combine the default target with all user specified targets
            targets: Targets::from_default_and_user_defined(
                toml_manifest.default_target,
                toml_manifest.target,
            ),
        };

        // Construct a default environment
        let default_environment = Environment {
            name: EnvironmentName::Default,
            features: Vec::new().into(),
            solve_group: None,
        };

        Ok(Self {
            project: toml_manifest.project,
            features: IndexMap::from_iter([(FeatureName::Default, default_feature)]),
            environments: IndexMap::from_iter([(EnvironmentName::Default, default_environment)]),
        })
    }
}

impl ProjectManifest {
    /// Validate the
    pub fn validate(&self, source: NamedSource, root_folder: &Path) -> miette::Result<()> {
        // Check if the targets are defined for existing platforms
        for feature in self.features.values() {
            let platforms = feature
                .platforms
                .as_ref()
                .unwrap_or(&self.project.platforms);
            for target_sel in feature.targets.user_defined_selectors() {
                match target_sel {
                    TargetSelector::Platform(p) => {
                        if !platforms.as_ref().contains(p) {
                            return Err(create_unsupported_platform_report(
                                source,
                                feature.targets.source_loc(target_sel).unwrap_or_default(),
                                p,
                                feature,
                            ));
                        }
                    }
                }
            }
        }

        // parse the SPDX license expression to make sure that it is a valid expression.
        if let Some(spdx_expr) = &self.project.license {
            spdx::Expression::parse(spdx_expr)
                .into_diagnostic()
                .with_context(|| {
                    format!(
                        "failed to parse the SPDX license expression '{}'",
                        spdx_expr
                    )
                })?;
        }

        let check_file_existence = |x: &Option<PathBuf>| {
            if let Some(path) = x {
                let full_path = root_folder.join(path);
                if !full_path.exists() {
                    return Err(miette::miette!(
                        "the file '{}' does not exist",
                        full_path.display()
                    ));
                }
            }
            Ok(())
        };

        check_file_existence(&self.project.license_file)?;
        check_file_existence(&self.project.readme)?;

        Ok(())
    }
}

/// Describes the contents of the `[package]` section of the project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMetadata {
    /// The name of the project
    pub name: String,

    /// The version of the project
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub version: Option<Version>,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    #[serde(default)]
    pub authors: Vec<String>,

    /// The channels used by the project
    #[serde_as(deserialize_as = "Vec<super::serde::ChannelStr>")]
    pub channels: Vec<Channel>,

    /// The platforms this project supports
    // TODO: This is actually slightly different from the rattler_conda_types::Platform because it
    //     should not include noarch.
    pub platforms: PixiSpanned<Vec<Platform>>,

    /// The license as a valid SPDX string (e.g. MIT AND Apache-2.0)
    pub license: Option<String>,

    /// The license file (relative to the project root)
    #[serde(rename = "license-file")]
    pub license_file: Option<PathBuf>,

    /// Path to the README file of the project (relative to the project root)
    pub readme: Option<PathBuf>,

    /// URL of the project homepage
    pub homepage: Option<Url>,

    /// URL of the project source repository
    pub repository: Option<Url>,

    /// URL of the project documentation
    pub documentation: Option<Url>,
}

#[serde_as]
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SystemRequirements {
    pub windows: Option<bool>,

    pub unix: Option<bool>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub macos: Option<Version>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub linux: Option<Version>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub cuda: Option<Version>,

    pub libc: Option<LibCSystemRequirement>,

    pub archspec: Option<String>,
}

impl SystemRequirements {
    pub fn virtual_packages(&self) -> Vec<VirtualPackage> {
        let mut result = Vec::new();
        if self.windows == Some(true) {
            result.push(VirtualPackage::Win);
        }
        if self.unix == Some(true) {
            result.push(VirtualPackage::Unix);
        }
        if let Some(version) = self.linux.clone() {
            result.push(VirtualPackage::Linux(Linux { version }));
        }
        if let Some(version) = self.cuda.clone() {
            result.push(VirtualPackage::Cuda(Cuda { version }));
        }
        if let Some(version) = self.macos.clone() {
            result.push(VirtualPackage::Osx(Osx { version }))
        }
        if let Some(spec) = self.archspec.clone() {
            result.push(VirtualPackage::Archspec(Archspec { spec }))
        }
        if let Some(libc) = self.libc.clone() {
            result.push(VirtualPackage::LibC(libc.into()))
        }

        result
    }
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LibCSystemRequirement {
    /// Only a version was specified, we assume glibc.
    GlibC(#[serde_as(as = "DisplayFromStr")] Version),

    /// Specified both a family and a version.
    OtherFamily(LibCFamilyAndVersion),
}

impl LibCSystemRequirement {
    /// Returns the family and version of this libc requirement.
    pub fn family_and_version(&self) -> (&str, &Version) {
        match self {
            LibCSystemRequirement::GlibC(version) => ("glibc", version),
            LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion { family, version: v }) => {
                (family.as_deref().unwrap_or("glibc"), v)
            }
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibCFamilyAndVersion {
    /// The libc family, e.g. glibc
    pub family: Option<String>,

    /// The minimum version of the libc family
    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,
}

impl From<LibCSystemRequirement> for LibC {
    fn from(value: LibCSystemRequirement) -> Self {
        match value {
            LibCSystemRequirement::GlibC(version) => LibC {
                version,
                family: String::from("glibc"),
            },
            LibCSystemRequirement::OtherFamily(libc) => libc.into(),
        }
    }
}

impl From<LibCFamilyAndVersion> for LibC {
    fn from(value: LibCFamilyAndVersion) -> Self {
        LibC {
            version: value.version,
            family: value.family.unwrap_or_else(|| String::from("glibc")),
        }
    }
}
#[derive(Default, Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub scripts: Option<Vec<String>>,
}

// Create an error report for using a platform that is not supported by the project.
fn create_unsupported_platform_report(
    source: NamedSource,
    span: Range<usize>,
    platform: &Platform,
    feature: &Feature,
) -> Report {
    miette::miette!(
        labels = vec![LabeledSpan::at(
            span,
            format!("'{}' is not a supported platform", platform)
        )],
        help = format!(
            "Add '{platform}' to the `{}` array of the {} manifest.",
            consts::PROJECT_MANIFEST,
            if feature.platforms.is_some() {
                format!(
                    "feature.{}.platforms",
                    feature
                        .name
                        .name()
                        .expect("default feature never defines custom platforms")
                )
            } else {
                String::from("project.platforms")
            }
        ),
        "targeting a platform that this project does not support"
    )
    .with_source_code(source)
}

#[cfg(test)]
mod test {
    use super::*;
    use insta::assert_display_snapshot;
    use std::str::FromStr;
    use tempfile::tempdir;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = []
        "#;

    #[test]
    fn test_from_path() {
        // Test the toml from a path
        let dir = tempdir().unwrap();
        let path = dir.path().join("pixi.toml");
        std::fs::write(&path, PROJECT_BOILERPLATE).unwrap();
        // From &PathBuf
        let _manifest = Manifest::from_path(&path).unwrap();
        // From &Path
        let _manifest = Manifest::from_path(path.as_path()).unwrap();
        // From PathBuf
        let manifest = Manifest::from_path(path).unwrap();

        assert_eq!(manifest.parsed.project.name, "foo");
        assert_eq!(
            manifest.parsed.project.version,
            Some(Version::from_str("0.1.0").unwrap())
        );
    }

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

        let manifest = ProjectManifest::from_toml_str(&contents).unwrap();
        let targets = &manifest.default_feature().targets;
        assert_eq!(
            targets.user_defined_selectors().cloned().collect_vec(),
            vec![
                TargetSelector::Platform(Platform::Win64),
                TargetSelector::Platform(Platform::Osx64)
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

        let manifest = ProjectManifest::from_toml_str(&contents).unwrap();
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

        let manifest = ProjectManifest::from_toml_str(&contents).unwrap();
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

        assert_display_snapshot!(examples
            .into_iter()
            .map(|example| ProjectManifest::from_toml_str(&format!(
                "{PROJECT_BOILERPLATE}\n{example}"
            ))
            .unwrap_err()
            .to_string())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[test]
    fn test_activation_scripts() {
        let contents = r#"
            [project]
            name = "foo"
            channels = []
            platforms = ["win-64", "linux-64"]

            [activation]
            scripts = [".pixi/install/setup.sh"]

            [target.win-64.activation]
            scripts = [".pixi/install/setup.ps1"]

            [target.linux-64.activation]
            scripts = [".pixi/install/setup.sh", "test"]
            "#;

        let manifest = Manifest::from_str(Path::new(""), contents).unwrap();
        let default_activation_scripts = manifest
            .default_feature()
            .targets
            .default()
            .activation
            .as_ref()
            .and_then(|a| a.scripts.as_ref());
        let win64_activation_scripts = manifest
            .default_feature()
            .targets
            .for_target(&TargetSelector::Platform(Platform::Win64))
            .unwrap()
            .activation
            .as_ref()
            .and_then(|a| a.scripts.as_ref());
        let linux64_activation_scripts = manifest
            .default_feature()
            .targets
            .for_target(&TargetSelector::Platform(Platform::Linux64))
            .unwrap()
            .activation
            .as_ref()
            .and_then(|a| a.scripts.as_ref());

        assert_eq!(
            default_activation_scripts,
            Some(&vec![String::from(".pixi/install/setup.sh")])
        );
        assert_eq!(
            win64_activation_scripts,
            Some(&vec![String::from(".pixi/install/setup.ps1")])
        );
        assert_eq!(
            linux64_activation_scripts,
            Some(&vec![
                String::from(".pixi/install/setup.sh"),
                String::from("test")
            ])
        );
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

        let manifest = ProjectManifest::from_toml_str(&contents).unwrap();

        assert_display_snapshot!(manifest
            .default_feature()
            .targets
            .iter()
            .flat_map(|(target, selector)| {
                let selector_name =
                    selector.map_or_else(|| String::from("default"), ToString::to_string);
                target.tasks.iter().filter_map(move |(name, task)| {
                    Some(format!(
                        "{}/{name} = {}",
                        &selector_name,
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

        assert_display_snapshot!(toml_edit::de::from_str::<ProjectManifest>(&contents)
            .expect("parsing should succeed!")
            .default_feature()
            .targets
            .default()
            .pypi_dependencies
            .clone()
            .into_iter()
            .flat_map(|d| d.into_iter())
            .map(|(name, spec)| format!(
                "{} = {}",
                name.as_source_str(),
                Item::from(spec).to_string()
            ))
            .join("\n"));
    }

    #[test]
    fn system_requirements_works() {
        let file_content = r#"
        windows = true
        unix = true
        linux = "5.11"
        cuda = "12.2"
        macos = "10.15"
        archspec = "arm64"
        libc = { family = "glibc", version = "2.12" }
        "#;

        let system_requirements: SystemRequirements =
            toml_edit::de::from_str(file_content).unwrap();

        let expected_requirements: Vec<VirtualPackage> = vec![
            VirtualPackage::Win,
            VirtualPackage::Unix,
            VirtualPackage::Linux(Linux {
                version: Version::from_str("5.11").unwrap(),
            }),
            VirtualPackage::Cuda(Cuda {
                version: Version::from_str("12.2").unwrap(),
            }),
            VirtualPackage::Osx(Osx {
                version: Version::from_str("10.15").unwrap(),
            }),
            VirtualPackage::Archspec(Archspec {
                spec: "arm64".to_string(),
            }),
            VirtualPackage::LibC(LibC {
                version: Version::from_str("2.12").unwrap(),
                family: "glibc".to_string(),
            }),
        ];

        assert_eq!(
            system_requirements.virtual_packages(),
            expected_requirements
        );
    }

    #[test]
    fn test_system_requirements_failing_edge_cases() {
        let file_contents = [
            r#"
        [system-requirements]
        libc = { verion = "2.12" }
        "#,
            r#"
        [system-requirements]
        lib = "2.12"
        "#,
            r#"
        [system-requirements.libc]
        version = "2.12"
        fam = "glibc"
        "#,
            r#"
        [system-requirements.lic]
        version = "2.12"
        family = "glibc"
        "#,
        ];

        for file_content in file_contents {
            let file_content = format!("{PROJECT_BOILERPLATE}\n{file_content}");
            assert!(toml_edit::de::from_str::<ProjectManifest>(&file_content).is_err());
        }
    }

    fn test_remove(file_contents: &str, name: &str, kind: SpecType, platform: Option<Platform>) {
        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        // Initially the dependency should exist
        assert!(manifest
            .default_feature()
            .targets
            .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
            .unwrap()
            .dependencies
            .get(&kind)
            .unwrap()
            .get(name)
            .is_some());

        // Remove the dependency from the manifest
        manifest
            .remove_dependency(&PackageName::new_unchecked(name), kind, platform)
            .unwrap();

        // The dependency should no longer exist
        assert!(manifest
            .default_feature()
            .targets
            .for_opt_target(platform.map(TargetSelector::Platform).as_ref())
            .unwrap()
            .dependencies
            .get(&kind)
            .unwrap()
            .get(name)
            .is_none());

        // Write the toml to string and verify the content
        assert_display_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_remove_target_dependencies() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
            fooz = "*"

            [target.win-64.dependencies]
            bar = "*"

            [target.linux-64.build-dependencies]
            baz = "*"
        "#;

        test_remove(
            file_contents,
            "baz",
            SpecType::Build,
            Some(Platform::Linux64),
        );
        test_remove(file_contents, "bar", SpecType::Run, Some(Platform::Win64));
        test_remove(file_contents, "fooz", SpecType::Run, None);
    }

    #[test]
    fn test_remove_dependencies() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
            fooz = "*"

            [target.win-64.dependencies]
            fooz = "*"

            [target.linux-64.build-dependencies]
            fooz = "*"
        "#;

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        manifest
            .remove_dependency(&PackageName::new_unchecked("fooz"), SpecType::Run, None)
            .unwrap();

        // The dependency should be removed from the default feature
        assert!(manifest
            .default_feature()
            .targets
            .default()
            .run_dependencies()
            .map(|d| d.is_empty())
            .unwrap_or(true));

        // Should still contain the fooz dependency for the different platforms
        for (platform, kind) in [
            (Platform::Linux64, SpecType::Build),
            (Platform::Win64, SpecType::Run),
        ] {
            assert!(manifest
                .default_feature()
                .targets
                .for_target(&TargetSelector::Platform(platform))
                .unwrap()
                .dependencies
                .get(&kind)
                .into_iter()
                .flat_map(|x| x.keys())
                .any(|x| x.as_normalized() == "fooz"));
        }
    }

    #[test]
    fn test_set_version() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
        "#;

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.version.as_ref().unwrap().clone(),
            Version::from_str("0.1.0").unwrap()
        );

        manifest.set_version(&String::from("1.2.3")).unwrap();

        assert_eq!(
            manifest.parsed.project.version.as_ref().unwrap().clone(),
            Version::from_str("1.2.3").unwrap()
        );
    }

    #[test]
    fn test_set_description() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
        "#;

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(
            manifest
                .parsed
                .project
                .description
                .as_ref()
                .unwrap()
                .clone(),
            String::from("foo description")
        );

        manifest
            .set_description(&String::from("my new description"))
            .unwrap();

        assert_eq!(
            manifest
                .parsed
                .project
                .description
                .as_ref()
                .unwrap()
                .clone(),
            String::from("my new description")
        );
    }

    #[test]
    fn test_add_platforms() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
        "#;

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64]
        );

        manifest.add_platforms([Platform::OsxArm64].iter()).unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64, Platform::OsxArm64]
        );
    }

    #[test]
    fn test_remove_platforms() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
        "#;

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Linux64, Platform::Win64]
        );

        manifest.remove_platforms(&vec!["linux-64"]).unwrap();

        assert_eq!(
            manifest.parsed.project.platforms.value,
            vec![Platform::Win64]
        );
    }

    #[test]
    fn test_add_channels() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = []
            platforms = ["linux-64", "win-64"]

            [dependencies]
        "#;

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(manifest.parsed.project.channels, vec![]);

        manifest.add_channels(["conda-forge"].iter()).unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap()]
        );
    }

    #[test]
    fn test_remove_channels() {
        // Using known files in the project so the test succeed including the file check.
        let file_contents = r#"
            [project]
            name = "foo"
            version = "0.1.0"
            description = "foo description"
            channels = ["conda-forge"]
            platforms = ["linux-64", "win-64"]

            [dependencies]
        "#;

        let mut manifest = Manifest::from_str(Path::new(""), file_contents).unwrap();

        assert_eq!(
            manifest.parsed.project.channels,
            vec![Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap()]
        );

        manifest.remove_channels(["conda-forge"].iter()).unwrap();

        assert_eq!(manifest.parsed.project.channels, vec![]);
    }
}
