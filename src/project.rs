use crate::consts;
use anyhow::{bail, Context};
use enum_iterator::{all, Sequence};
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, NamelessMatchSpec, Platform, Version,
};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use serde::{de::IntoDeserializer, Deserialize};
use std::{
    collections::HashMap,
    env, fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};
use toml_edit::{Document, Item, Table, Value};

/// Enum representing supported system requirements.
/// Used for compile-time mapping and enhanced error reporting.
#[derive(Debug, Sequence)]
pub enum SystemRequirementKey {
    Windows,
    Unix,
    Linux,
    MacOS,
    Cuda,
    ArchSpec,
    LibC,
}

impl fmt::Display for SystemRequirementKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SystemRequirementKey::Windows => write!(f, "windows"),
            SystemRequirementKey::Unix => write!(f, "unix"),
            SystemRequirementKey::Linux => write!(f, "linux"),
            SystemRequirementKey::MacOS => write!(f, "macos"),
            SystemRequirementKey::Cuda => write!(f, "cuda"),
            SystemRequirementKey::ArchSpec => write!(f, "archspec"),
            SystemRequirementKey::LibC => write!(f, "libc"),
        }
    }
}

impl FromStr for SystemRequirementKey {
    type Err = &'static str;
    fn from_str(key: &str) -> Result<Self, Self::Err> {
        match key {
            "windows" => Ok(SystemRequirementKey::Windows),
            "unix" => Ok(SystemRequirementKey::Unix),
            "linux" => Ok(SystemRequirementKey::Linux),
            "macos" => Ok(SystemRequirementKey::MacOS),
            "cuda" => Ok(SystemRequirementKey::Cuda),
            "archspec" => Ok(SystemRequirementKey::ArchSpec),
            "libc" => Ok(SystemRequirementKey::LibC),
            _ => Err("Invalid system requirement"),
        }
    }
}

impl SystemRequirementKey {
    fn parse_requirements(&self, item: &Item) -> anyhow::Result<Option<VirtualPackage>> {
        match self {
            SystemRequirementKey::Windows => parse_windows_system_requirements(item),
            SystemRequirementKey::Unix => parse_unix_system_requirements(item),
            SystemRequirementKey::Linux => parse_linux_system_requirements(item),
            SystemRequirementKey::MacOS => parse_macos_system_requirements(item),
            SystemRequirementKey::Cuda => parse_cuda_system_requirements(item),
            SystemRequirementKey::ArchSpec => parse_archspec_system_requirements(item),
            SystemRequirementKey::LibC => parse_libc_system_requirements(item),
        }
    }
}
/// A project represented by a pixi.toml file.
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
        let deps = self
            .doc
            .get("dependencies")
            .ok_or_else(|| {
                anyhow::anyhow!("No dependencies found in {}", consts::PROJECT_MANIFEST)
            })?
            .as_table_like()
            .ok_or_else(|| {
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

    pub fn name(&self) -> anyhow::Result<&str> {
        return self.doc["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No name found in {}", consts::PROJECT_MANIFEST));
    }

    pub fn version(&self) -> anyhow::Result<&str> {
        return self.doc["version"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No version found in {}", consts::PROJECT_MANIFEST));
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

    /// Get the command with the specified name or `None` if no such command exists.
    pub fn command_opt(&self, name: &str) -> anyhow::Result<Option<crate::script::Command>> {
        if let Some(command) = self
            .doc
            .get("commands")
            .and_then(|x| x.as_table_like())
            .and_then(|tbl| tbl.get(name))
        {
            let script = match command.clone().into_table() {
                Ok(table) => {
                    let de = Document::from(table).into_deserializer();
                    crate::script::Command::deserialize(de)?
                }
                Err(command) => match command.into_value() {
                    Ok(value) => {
                        let de = value.into_deserializer();
                        crate::script::Command::deserialize(de)?
                    }
                    Err(_) => {
                        anyhow::bail!("could not convert TOML to command")
                    }
                },
            };

            Ok(Some(script))
        } else {
            Ok(None)
        }
    }

    /// Get the system requirements defined under the `system-requirements` section of the project manifest.
    /// These get turned into virtual packages which are used in the solve.
    /// They will act as the description of a reference machine which is minimally needed for this package to be run.
    pub fn system_requirements(&self) -> anyhow::Result<Vec<VirtualPackage>> {
        let mut res = vec![];

        // If some system requirements are defined, commit them
        if let Some(sys_req_table) = self
            .doc
            .get("system-requirements")
            .and_then(|x| x.as_table_like())
        {
            for (key, item) in sys_req_table.iter() {
                let req = SystemRequirementKey::from_str(key);
                match req {
                    Ok(requirement) => {
                        if let Some(pkg) = requirement.parse_requirements(item)? {
                            res.push(pkg);
                        }
                    }
                    // handle other cases
                    _ => bail!(
                    "'{}' is an unknown system-requirement, please use one of the following: {}.",
                    key,
                    all::<SystemRequirementKey>().collect::<Vec<_>>().iter().map(|k| format!("{}", k)).collect::<Vec<_>>().join(", ")
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
// Parse windows virtual package from the system requirement input.
fn parse_windows_system_requirements(item: &Item) -> anyhow::Result<Option<VirtualPackage>> {
    let windows = item
        .as_bool()
        .ok_or(anyhow::anyhow!("expected boolean value for windows"))?;
    if windows {
        Ok(Some(VirtualPackage::Win))
    } else {
        Ok(None)
    }
}
// Parse unix virtual package from the system requirement input.
fn parse_unix_system_requirements(item: &Item) -> anyhow::Result<Option<VirtualPackage>> {
    let unix = item
        .as_bool()
        .ok_or(anyhow::anyhow!("expected boolean value for unix"))?;
    if unix {
        Ok(Some(VirtualPackage::Unix))
    } else {
        Ok(None)
    }
}
// Parse macos virtual package from the system requirement input.
fn parse_macos_system_requirements(item: &Item) -> anyhow::Result<Option<VirtualPackage>> {
    let macos_version = item
        .as_str()
        .ok_or(anyhow::anyhow!("expected string value for macos"))?;
    Ok(Some(VirtualPackage::Osx(Osx {
        version: Version::from_str(macos_version).unwrap(),
    })))
}
// Parse linux virtual package from the system requirement input.
fn parse_linux_system_requirements(item: &Item) -> anyhow::Result<Option<VirtualPackage>> {
    let linux_version = item
        .as_str()
        .ok_or(anyhow::anyhow!("expected string value for linux"))?;
    Ok(Some(VirtualPackage::Linux(Linux {
        version: Version::from_str(linux_version).unwrap(),
    })))
}
// Parse cuda virtual package from the system requirement input.
fn parse_cuda_system_requirements(item: &Item) -> anyhow::Result<Option<VirtualPackage>> {
    let cuda_version = item
        .as_str()
        .ok_or(anyhow::anyhow!("expected string value for cuda"))?;
    Ok(Some(VirtualPackage::Cuda(Cuda {
        version: Version::from_str(cuda_version).unwrap(),
    })))
}
// Parse archspec virtual package from the system requirement input.
fn parse_archspec_system_requirements(item: &Item) -> anyhow::Result<Option<VirtualPackage>> {
    let archspec_version = item
        .as_str()
        .ok_or(anyhow::anyhow!("expected string value for archspec"))?;
    Ok(Some(VirtualPackage::Archspec(Archspec {
        spec: archspec_version.to_string(),
    })))
}

// Parse libc virtual package from the system requirement input.
fn parse_libc_system_requirements(item: &Item) -> anyhow::Result<Option<VirtualPackage>> {
    match item {
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
            Ok(Some(VirtualPackage::LibC(LibC { family, version })))
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
                Ok(Some(VirtualPackage::LibC(LibC { family, version })))
            }
            Value::String(version) => Ok(Some(VirtualPackage::LibC(LibC {
                family: "glibc".to_string(),
                version: Version::from_str(version.value())?,
            }))),
            _ => bail!("expected version string or table as value for libc"),
        },
        _ => bail!("expected version string or table as value for libc"),
    }
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
            linux = "5.11"
            cuda = "12.2"
            macos = "10.15"
            archspec = "arm64"
            libc = { family = "glibc", version = "2.12" }
        "#;

        let project = Project {
            root: PathBuf::from(""),
            doc: Document::from_str(file_content).unwrap(),
        };

        let system_requirements = project.system_requirements().unwrap();

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
