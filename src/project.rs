use crate::consts;
use anyhow::{bail, Context};
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, NamelessMatchSpec, Platform, Version,
};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Osx, VirtualPackage};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{env, fs};
use toml_edit::{Document, Item, Table, Value};

/// A project represented by a pax.toml file.
#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    doc: Document,
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
        let doc = fs::read_to_string(filename)?
            .parse::<Document>()
            .with_context(|| {
                format!(
                    "failed to parse {} from {}",
                    consts::PROJECT_MANIFEST,
                    filename.display()
                )
            })?;

        Ok(Self {
            root: root.to_path_buf(),
            doc,
        })
    }

    pub fn dependencies(&self) -> anyhow::Result<HashMap<String, NamelessMatchSpec>> {
        let deps = self.doc["dependencies"].as_table_like().ok_or_else(|| {
            anyhow::anyhow!("dependencies in {} are malformed", consts::PROJECT_MANIFEST)
        })?;

        let mut result = HashMap::with_capacity(deps.len());
        for (name, value) in deps.iter() {
            let match_spec = value
                .as_str()
                .map(|str| NamelessMatchSpec::from_str(str).map_err(Into::into))
                .unwrap_or_else(|| {
                    Err(anyhow::anyhow!(
                        "dependencies in {} are malformed",
                        consts::PROJECT_MANIFEST
                    ))
                })?;
            result.insert(name.to_owned(), match_spec);
        }

        Ok(result)
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
        let spec_string = spec.to_string();
        let requirement = spec_string.split_once(' ').unwrap_or(("", "*")).1;

        // Store (or replace) in the document
        deps_table.insert(name, Item::Value(requirement.into()));

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
    pub fn channels(&self, channel_config: &ChannelConfig) -> anyhow::Result<Vec<Channel>> {
        let channels_array = self
            .doc
            .get("project")
            .and_then(|x| x.get("channels"))
            .and_then(|x| x.as_array())
            .ok_or_else(|| anyhow::anyhow!("malformed or missing 'channels'"))?;

        let mut channels = Vec::with_capacity(channels_array.len());
        for channel_item in channels_array {
            let channel_str = channel_item
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("malformed channel"))?;
            let channel = Channel::from_str(channel_str, channel_config)?;
            channels.push(channel);
        }

        Ok(channels)
    }

    /// Returns the platforms this project targets
    pub fn platforms(&self) -> anyhow::Result<Vec<Platform>> {
        let platforms_array = self
            .doc
            .get("project")
            .and_then(|x| x.get("platforms"))
            .and_then(|x| x.as_array())
            .ok_or_else(|| anyhow::anyhow!("malformed or missing 'platforms'"))?;

        let mut platforms = Vec::with_capacity(platforms_array.len());
        for platform_item in platforms_array {
            let platform_str = platform_item
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("malformed platform"))?;
            let platform = Platform::from_str(platform_str)?;
            platforms.push(platform);
        }

        Ok(platforms)
    }

    /// Get the commands defined under the `commands` section of the project manifest.
    pub fn commands(&self) -> anyhow::Result<HashMap<String, String>> {
        let mut res = HashMap::new();

        // If some commands are defined commit them otherwise return empty map
        if let Some(commands_table) = self.doc.get("commands").and_then(|x| x.as_table_like()) {
            for (key, val) in commands_table.iter() {
                let command_str = val
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("malformed command"))?;
                res.insert(key.to_string(), command_str.to_string());
            }
        }

        Ok(res)
    }

    pub fn system_requirements(&self) -> anyhow::Result<Vec<VirtualPackage>> {
        let mut res = vec![];

        // If some system requirements are defined, commit them
        if let Some(sys_req_table) = self
            .doc
            .get("system-requirements")
            .and_then(|x| x.as_table_like())
        {
            for (key, val) in sys_req_table.iter() {
                match key {
                    "windows" => {
                        let windows = val
                            .as_bool()
                            .ok_or(anyhow::anyhow!("expected boolean value for windows"))?;
                        if windows {
                            res.push(VirtualPackage::Win);
                        }
                    }
                    "unix" => {
                        let unix = val
                            .as_bool()
                            .ok_or(anyhow::anyhow!("expected boolean value for unix"))?;
                        if unix {
                            res.push(VirtualPackage::Unix);
                        }
                    }
                    "macos" => {
                        let macos_version = val
                            .as_str()
                            .ok_or(anyhow::anyhow!("expected string value for macos"))?
                            .to_owned();
                        res.push(VirtualPackage::Osx(Osx {
                            version: Version::from_str(macos_version.as_str()).unwrap(),
                        }));
                    }
                    "cuda" => {
                        let cuda_version = val
                            .as_str()
                            .ok_or(anyhow::anyhow!("expected string value for cuda"))?
                            .to_owned();
                        res.push(VirtualPackage::Cuda(Cuda {
                            version: Version::from_str(cuda_version.as_str()).unwrap(),
                        }));
                    }
                    "archspec" => {
                        let spec = val
                            .as_str()
                            .ok_or(anyhow::anyhow!("expected string value for archspec"))?
                            .to_owned();
                        res.push(VirtualPackage::Archspec(Archspec { spec }));
                    }
                    "libc" => match val {
                        Item::Table(table) => {
                            let family: String = table
                                .get("family")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_owned())
                                .unwrap_or_else(|| String::from("glibc"));
                            let version_str = table
                                .get("version")
                                .and_then(|v| v.as_str())
                                .ok_or(anyhow::anyhow!("missing or invalid 'version'"))?;
                            let version = Version::from_str(version_str)?;
                            // Check for other keys
                            for (key, _) in table.iter() {
                                if key != "family" && key != "version" {
                                    return Err(anyhow::anyhow!("Unexpected key in 'libc' table: {}", key));
                                }
                            }
                            res.push(VirtualPackage::LibC(LibC { family, version }));
                        }
                        Item::Value(value) => match value {
                            Value::InlineTable(inline) => {
                                let family: String = inline
                                    .get("family")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_owned())
                                    .unwrap_or_else(|| String::from("glibc"));
                                let version_str = inline
                                    .get("version")
                                    .and_then(|v| v.as_str())
                                    .ok_or(anyhow::anyhow!("missing or invalid 'version'"))?;
                                let version = Version::from_str(version_str)?;
                                // check for other keys
                                for (key, _) in inline.iter() {
                                    if key != "family" && key != "version" {
                                        return Err(anyhow::anyhow!("Unexpected key in 'libc' table: {}", key));
                                    }
                                }
                                res.push(VirtualPackage::LibC(LibC { family, version }));
                            }
                            Value::String(version) => {
                                res.push(VirtualPackage::LibC(LibC {
                                    family: "glibc".to_string(),
                                    version: Version::from_str(version.value())?,
                                }));
                            }
                            _ => bail!("expected version string or table as value for libc"),
                        },
                        _ => bail!("expected version string or table as value for libc"),
                    },
                    // handle other cases
                    _ => bail!(
                        "'{}' is an unknown system-requirement, please use one of the defaults.",
                        key
                    ),
                }
            }
        }
        Ok(res)
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
    use rattler_virtual_packages::{Cuda, Osx, VirtualPackage};

    #[test]
    fn system_requirements_works() {
        let file_content = r#"
            [system-requirements]
            windows = true
            unix = true
            cuda = "12.2"
            macos = "10.15"
            libc = { family = "glibc", version = "2.12" }
        "#;

        let project = Project {
            root: PathBuf::from(""),
            doc: Document::from_str(file_content).unwrap(),
        };

        let system_requirements = project.system_requirements().unwrap();
        println!("{:?}", system_requirements);

        let mut expected_requirements: Vec<VirtualPackage> = vec![];
        expected_requirements.push(VirtualPackage::Win);
        expected_requirements.push(VirtualPackage::Unix);
        expected_requirements.push(VirtualPackage::Cuda(Cuda {
            version: Version::from_str("12.2").unwrap(),
        }));
        expected_requirements.push(VirtualPackage::Osx(Osx {
            version: Version::from_str("10.15").unwrap(),
        }));
        expected_requirements.push(VirtualPackage::LibC(LibC {
            version: Version::from_str("2.12").unwrap(),
            family: "glibc".to_string(),
        }));

        assert_eq!(system_requirements, expected_requirements);
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
            let project = Project {
                root: PathBuf::from(""),
                doc: Document::from_str(file_content).unwrap(),
            };

            let expected_result = vec![VirtualPackage::LibC(LibC {
                family: "glibc".to_string(),
                version: Version::from_str("2.12").unwrap(),
            })];

            let system_requirements = project.system_requirements().unwrap();

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
            let project = Project {
                root: PathBuf::from(""),
                doc: Document::from_str(file_content).unwrap(),
            };
            assert!(project.system_requirements().is_err());
        }
    }
}
