pub mod environment;
mod manifest;
mod serde;

use crate::consts;
use crate::consts::PROJECT_MANIFEST;
use crate::project::manifest::{ProjectManifest, TargetMetadata, TargetSelector};
use crate::report_error::ReportError;
use anyhow::Context;
use ariadne::{Label, Report, ReportKind, Source};
use rattler_conda_types::{Channel, MatchSpec, NamelessMatchSpec, Platform, Version};
use rattler_virtual_packages::VirtualPackage;
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};
use toml_edit::{Document, Item, Table, TomlError};

/// A project represented by a pixi.toml file.
#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    pub(crate) source: String,
    doc: Document,
    pub(crate) manifest: ProjectManifest,
}

impl Project {
    /// Discovers the project manifest file in the current directory or any of the parent
    /// directories.
    pub fn discover() -> anyhow::Result<Self> {
        let project_toml = match find_project_root() {
            Some(root) => root.join(consts::PROJECT_MANIFEST),
            None => anyhow::bail!("could not find {}", consts::PROJECT_MANIFEST),
        };
        Self::load(&project_toml)
    }

    /// Loads a project manifest file.
    pub fn load(filename: &Path) -> anyhow::Result<Self> {
        // Determine the parent directory of the manifest file
        let root = filename.parent().unwrap_or(Path::new("."));

        // Load the TOML document
        Self::from_manifest_str(root, fs::read_to_string(filename)?).with_context(|| {
            format!(
                "failed to parse {} from {}",
                consts::PROJECT_MANIFEST,
                root.display()
            )
        })
    }

    /// Loads a project manifest.
    pub fn from_manifest_str(root: &Path, contents: impl Into<String>) -> anyhow::Result<Self> {
        let contents = contents.into();
        let (manifest, doc) = match toml_edit::de::from_str::<ProjectManifest>(&contents)
            .map_err(TomlError::from)
            .and_then(|manifest| contents.parse::<Document>().map(|doc| (manifest, doc)))
        {
            Ok(result) => result,
            Err(e) => {
                if let Some(span) = e.span() {
                    return Err(ReportError {
                        source: (PROJECT_MANIFEST, Source::from(&contents)),
                        report: Report::build(ReportKind::Error, PROJECT_MANIFEST, span.start)
                            .with_message("failed to parse project manifest")
                            .with_label(
                                Label::new((PROJECT_MANIFEST, span)).with_message(e.message()),
                            )
                            .finish(),
                    }
                    .into());
                } else {
                    return Err(e.into());
                }
            }
        };

        // Validate the contents of the manifest
        manifest.validate(&contents)?;

        Ok(Self {
            root: root.to_path_buf(),
            source: contents,
            doc,
            manifest,
        })
    }

    /// Returns the dependencies of the project.
    pub fn dependencies(
        &self,
        platform: Platform,
    ) -> anyhow::Result<HashMap<String, NamelessMatchSpec>> {
        // Get the base dependencies (defined in the `[dependencies]` section)
        let base_dependencies = self.manifest.dependencies.iter();

        // Get the platform specific dependencies in the order they were defined.
        let platform_specific = self
            .target_specific_metadata(platform)
            .flat_map(|target| target.dependencies.iter());

        // Combine the specs.
        //
        // Note that if a dependency was specified twice the platform specific one "wins".
        Ok(base_dependencies
            .chain(platform_specific)
            .map(|(name, spec)| (name.clone(), spec.clone()))
            .collect())
    }

    /// Returns all the targets specific metadata that apply with the given context.
    /// TODO: Add more context here?
    /// TODO: Should we return the selector too to provide better diagnostics later?
    pub fn target_specific_metadata(
        &self,
        platform: Platform,
    ) -> impl Iterator<Item = &'_ TargetMetadata> + '_ {
        self.manifest
            .target
            .iter()
            .filter_map(move |(selector, manifest)| match selector.as_ref() {
                TargetSelector::Platform(p) if p == &platform => Some(manifest),
                _ => None,
            })
    }

    /// Returns the name of the project
    pub fn name(&self) -> &str {
        &self.manifest.project.name
    }

    /// Returns the version of the project
    pub fn version(&self) -> &Version {
        &self.manifest.project.version
    }

    pub fn add_dependency(&mut self, spec: &MatchSpec) -> anyhow::Result<()> {
        // Find the dependencies table
        let deps = &mut self.doc["dependencies"];

        // If it doesnt exist create a proper table
        if deps.is_none() {
            *deps = Item::Table(Table::new());
        }

        // Cast the item into a table
        let deps_table = deps.as_table_like_mut().ok_or_else(|| {
            anyhow::anyhow!("dependencies in {} are malformed", consts::PROJECT_MANIFEST)
        })?;

        // Determine the name of the package to add
        let name = spec
            .name
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("* package specifier is not supported"))?;

        // Format the requirement
        // TODO: Do this smarter. E.g.:
        //  - split this into an object if exotic properties (like channel) are specified.
        //  - split the name from the rest of the requirement.
        let nameless = NamelessMatchSpec::from(spec.to_owned());

        // Store (or replace) in the document
        deps_table.insert(name, Item::Value(nameless.to_string().into()));

        self.manifest.dependencies.insert(name.to_string(), nameless);

        Ok(())
    }

    /// Returns the root directory of the project
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the path to the manifest file.
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(consts::PROJECT_MANIFEST)
    }

    /// Returns the path to the lock file of the project
    pub fn lock_file_path(&self) -> PathBuf {
        self.root.join(consts::PROJECT_LOCK_FILE)
    }

    /// Save back changes
    pub fn save(&self) -> anyhow::Result<()> {
        fs::write(self.manifest_path(), self.doc.to_string()).with_context(|| {
            format!(
                "unable to write changes to {}",
                self.manifest_path().display()
            )
        })?;
        Ok(())
    }

    /// Returns the channels used by this project
    pub fn channels(&self) -> &[Channel] {
        &self.manifest.project.channels
    }

    /// Returns the platforms this project targets
    pub fn platforms(&self) -> &[Platform] {
        self.manifest.project.platforms.as_ref().as_slice()
    }

    /// Get the command with the specified name or `None` if no such command exists.
    pub fn command_opt(&self, name: &str) -> Option<&crate::command::Command> {
        self.manifest.commands.get(name)
    }

    /// Get the system requirements defined under the `system-requirements` section of the project manifest.
    /// These get turned into virtual packages which are used in the solve.
    /// They will act as the description of a reference machine which is minimally needed for this package to be run.
    pub fn system_requirements(&self) -> Vec<VirtualPackage> {
        self.manifest.system_requirements.virtual_packages()
    }
}

/// Iterates over the current directory and all its parent directories and returns the first
/// directory path that contains the [`consts::PROJECT_MANIFEST`].
pub fn find_project_root() -> Option<PathBuf> {
    let current_dir = env::current_dir().ok()?;
    std::iter::successors(Some(current_dir.as_path()), |prev| prev.parent())
        .find(|dir| dir.join(consts::PROJECT_MANIFEST).is_file())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::manifest::SystemRequirements;
    use rattler_conda_types::ChannelConfig;
    use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
    use std::str::FromStr;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = []
        "#;

    #[test]
    fn test_main_project_config() {
        let file_content = r#"
            [project]
            name = "pixi"
            version = "0.0.2"
            channels = ["conda-forge"]
            platforms = ["linux-64", "win-64"]
        "#;

        let project = Project::from_manifest_str(Path::new(""), file_content.to_string()).unwrap();

        assert_eq!(project.name(), "pixi");
        assert_eq!(project.version(), &Version::from_str("0.0.2").unwrap());
        assert_eq!(
            project.channels(),
            [Channel::from_name(
                "conda-forge",
                None,
                &ChannelConfig::default()
            )]
        );
        assert_eq!(
            project.platforms(),
            [
                Platform::from_str("linux-64").unwrap(),
                Platform::from_str("win-64").unwrap()
            ]
        );
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

        let mut expected_requirements: Vec<VirtualPackage> = vec![];
        expected_requirements.push(VirtualPackage::Win);
        expected_requirements.push(VirtualPackage::Unix);
        expected_requirements.push(VirtualPackage::Linux(Linux {
            version: Version::from_str("5.11").unwrap(),
        }));
        expected_requirements.push(VirtualPackage::Cuda(Cuda {
            version: Version::from_str("12.2").unwrap(),
        }));
        expected_requirements.push(VirtualPackage::Osx(Osx {
            version: Version::from_str("10.15").unwrap(),
        }));
        expected_requirements.push(VirtualPackage::Archspec(Archspec {
            spec: "arm64".to_string(),
        }));
        expected_requirements.push(VirtualPackage::LibC(LibC {
            version: Version::from_str("2.12").unwrap(),
            family: "glibc".to_string(),
        }));

        assert_eq!(
            system_requirements.virtual_packages(),
            expected_requirements
        );
    }

    #[test]
    fn test_system_requirements_edge_cases() {
        let file_contents = [
            r#"
        [system-requirements]
        libc = { version = "2.12" }
        "#,
            r#"
        [system-requirements]
        libc = "2.12"
        "#,
            r#"
        [system-requirements.libc]
        version = "2.12"
        "#,
            r#"
        [system-requirements.libc]
        version = "2.12"
        family = "glibc"
        "#,
        ];

        for file_content in file_contents {
            let file_content = format!("{PROJECT_BOILERPLATE}\n{file_content}");

            let project = Project::from_manifest_str(Path::new(""), &file_content).unwrap();

            let expected_result = vec![VirtualPackage::LibC(LibC {
                family: "glibc".to_string(),
                version: Version::from_str("2.12").unwrap(),
            })];

            let system_requirements = project.system_requirements();

            assert_eq!(system_requirements, expected_result);
        }
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
}
