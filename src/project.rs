use crate::consts;
use anyhow::{bail, Context};
use rattler_conda_types::{Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, NamelessMatchSpec, ParseVersionError, Platform, Version};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{env, fs};
use toml_edit::{Document, Item, Table, Value};

/// A project represented by a pex.toml file.
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

    pub fn system_requirements(&self) -> anyhow::Result<HashMap<String, Vec<GenericVirtualPackage>>> {
        let mut  res = HashMap::new();

        // Read platform-independent requirements.
        if let Some(system_req_table) = self.doc.get("system-requirements").and_then(|x| x.as_table_like()){
            let mut platform_reqs = vec![];
            for (key, val) in system_req_table.iter() {

                let requirement = match val {
                    Item::Value(value) => {
                        match value {
                            Value::String(value) => GenericVirtualPackage {
                                name: key.to_string(),
                                version: Version::from_str(value.value()).unwrap(),
                                build_string: "".to_string(),
                            },
                            Value::Integer(value) => GenericVirtualPackage {
                                name: key.to_string(),
                                version: Version::from_str(&value.to_string()).unwrap(),
                                build_string: "".to_string(),
                            },
                            Value::Float(value) => GenericVirtualPackage {
                                name: key.to_string(),
                                version: Version::from_str(&value.to_string()).unwrap(),
                                build_string: "".to_string(),
                            },
                            Value::InlineTable(t) => {
                                let version = t.get("version").and_then(|x| x.as_str()).unwrap_or_default();
                                let build_string = t.get("build_string").and_then(|x| x.as_str()).map(String::from);
                                GenericVirtualPackage {
                                    name: key.to_string(),
                                    version: Version::from_str(version).unwrap(),
                                    build_string: build_string.unwrap_or("".to_string()),
                                }
                            }
                            _ => {bail!("The value of key: {} can not be rendered into a system-requirement", key.to_string())}
                        }
                                            }
                    ,
                    Item::Table(value) => {
                        if value.contains_key("version") || value.contains_key("build_string") {
                            let version = value.get("version").and_then(|x| x.as_str()).unwrap_or_default();
                            let build_string = value.get("build_string").and_then(|x| x.as_str()).map(String::from);
                            GenericVirtualPackage {
                                name: key.to_string(),
                                version: Version::from_str(version).unwrap(),
                                build_string: build_string.unwrap_or("".to_string()),
                            }
                        } else {
                            continue;
                        }
                    },
                    _ => continue,  // Skip values that are neither a simple value nor a table.
                };
                platform_reqs.push( requirement);
            }
            res.insert("default".to_string(), platform_reqs);
        }

        // Read platform-specific requirements.
        if let Some(system_req_table) = self.doc.get("system-requirements").and_then(|x| x.as_table()){
            for (platform, platform_table) in system_req_table.iter() {
                if let Some(pkg_table) = platform_table.as_table() {
                    let mut platform_reqs = vec![];
                    for (key, val) in pkg_table.iter() {
                        let requirement = match val {
                            Item::Value(value) => GenericVirtualPackage {
                                name: key.to_string(),
                                version: Version::from_str(value.as_str().unwrap_or_default()).unwrap(),
                                build_string: "".to_string(),
                            },
                            Item::Table(value) => {
                                let version = value.get("version").and_then(|x| x.as_str()).unwrap_or_default();
                                let build_string = value.get("build_string").and_then(|x| x.as_str()).map(String::from);
                                GenericVirtualPackage {
                                    name: key.to_string(),
                                    version: Version::from_str(version).unwrap(),
                                    build_string: build_string.unwrap_or("".to_string()),
                                }
                            },
                            _ => continue,  // Skip values that are neither a simple value nor a table.
                        };
                        platform_reqs.push(requirement);
                    }

                    res.insert(platform.to_string(), platform_reqs);
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

fn match_table_like_with_generic_virtual_package(){

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_requirements_works() {
        let file_content = r#"
            [system-requirements]
            __cuda = "11.4.1"
            __linux = {version = "5.1", build_string="musl"}

            [system-requirements.linux]
            __glibc = "2.17"
            __archspec = "1"
            __linux = {version = "5.2", build_string="arm64"}
        "#;

        let mut project = Project {
            root: PathBuf::from(""),
            doc: Document::from_str(file_content).unwrap(),
        };

        let system_requirements = project.system_requirements().unwrap();
        let expected_default_requirements = vec![
                GenericVirtualPackage {
                    name: "__cuda".to_string(),
                    version: Version::from_str("11.4").unwrap(),
                    build_string: "".to_string(),
                },
        ].into_iter().collect::<Vec<_>>();

        let expected_linux_requirements = vec![
                GenericVirtualPackage {
                    name: "__glibc".to_string(),
                    version: Version::from_str("2.17").unwrap(),
                    build_string: "".to_string(),
                },
                GenericVirtualPackage {
                    name: "__archspec".to_string(),
                    version: Version::from_str("1").unwrap(),
                    build_string: "".to_string(),
                },
                GenericVirtualPackage {
                    name: "__linux".to_string(),
                    version: Version::from_str("5.2").unwrap(),
                    build_string: "arm64".to_string(),
                },
        ].into_iter().collect::<Vec<_>>();

        assert_eq!(system_requirements.get("default").unwrap(), &expected_default_requirements);
        assert_eq!(system_requirements.get("linux").unwrap(), &expected_linux_requirements);
    }
}