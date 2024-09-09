use std::{
    env,
    ffi::OsStr,
    fmt::Formatter,
    path::{Path, PathBuf},
    str::FromStr,
    sync::OnceLock,
};

pub(crate) use environment::EnvironmentName;
use indexmap::IndexMap;
use itertools::Itertools;
use manifest::Manifest;
use miette::{miette, Context, IntoDiagnostic};
use once_cell::sync::Lazy;
pub(crate) use parsed_manifest::ExposedKey;
pub(crate) use parsed_manifest::ParsedEnvironment;
use parsed_manifest::ParsedManifest;
use pixi_config::{home_path, Config};
use pixi_manifest::PrioritizedChannel;
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform};
use rattler_repodata_gateway::Gateway;
use regex::Regex;
use reqwest_middleware::ClientWithMiddleware;
use std::fmt::Debug;

use crate::{
    global::{common::is_text, EnvDir},
    prefix::Prefix,
};

use super::{BinDir, EnvRoot};

mod document;
mod environment;
mod error;
mod manifest;
mod parsed_manifest;

pub(crate) const MANIFEST_DEFAULT_NAME: &str = "pixi-global.toml";

/// The pixi global project, this main struct to interact with the pixi global project.
/// This struct holds the `Manifest` and has functions to modify
/// or request information from it. This allows in the future to have multiple manifests
/// linked to a pixi global project.
#[derive(Clone)]
pub struct Project {
    /// Root folder of the project
    root: PathBuf,
    /// Reqwest client shared for this project.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    client: OnceLock<(reqwest::Client, ClientWithMiddleware)>,
    /// The repodata gateway to use for answering queries about repodata.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    repodata_gateway: OnceLock<Gateway>,
    /// The manifest for the project
    pub(crate) manifest: Manifest,
    /// The global configuration as loaded from the config file(s)
    config: Config,
}

impl Debug for Project {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Global Project")
            .field("root", &self.root)
            .field("manifest", &self.manifest)
            .finish()
    }
}

#[derive(Debug)]
struct ExposedData {
    env: EnvironmentName,
    platform: Option<Platform>,
    channel: PrioritizedChannel,
    package: PackageName,
    exposed: ExposedKey,
    binary: String,
}

impl Project {
    /// Constructs a new instance from an internal manifest representation
    fn from_manifest(manifest: Manifest) -> Self {
        let root = manifest
            .path
            .parent()
            .expect("manifest path should always have a parent")
            .to_owned();

        let config = Config::load(&root);

        Self {
            root,
            client: Default::default(),
            repodata_gateway: Default::default(),
            manifest,
            config,
        }
    }

    /// Constructs a project from a manifest.
    pub(crate) fn from_str(manifest_path: &Path, content: &str) -> miette::Result<Self> {
        let manifest = Manifest::from_str(manifest_path, content)?;
        Ok(Self::from_manifest(manifest))
    }

    /// Discovers the project manifest file in path at `~/.pixi/manifests/pixi-global.toml`.
    /// If the manifest doesn't exist yet, and the function will try to create one from the existing installation.
    /// If that one fails, an empty one will be created.
    pub(crate) async fn discover(
        bin_dir: &BinDir,
        env_root: &EnvRoot,
        assume_yes: bool,
    ) -> miette::Result<Self> {
        let manifest_dir = Self::manifest_dir()?;

        tokio::fs::create_dir_all(&manifest_dir)
            .await
            .into_diagnostic()
            .wrap_err_with(|| format!("Couldn't create directory {}", manifest_dir.display()))?;

        let manifest_path = manifest_dir.join(MANIFEST_DEFAULT_NAME);

        if !manifest_path.exists() {
            let warn = console::style(console::Emoji("⚠️ ", "")).yellow();
            let prompt = format!(
                "{} You don't have a global manifest yet.\n\
                Do you want to create one based on your existing installation?\n\
                Your existing installation will be removed if you decide against it.",
                console::style(console::Emoji("⚠️ ", "")).yellow(),
            );
            if !env_root.directories().await?.is_empty()
                && (assume_yes
                    || dialoguer::Confirm::new()
                        .with_prompt(prompt)
                        .default(true)
                        .show_default(true)
                        .interact()
                        .into_diagnostic()?)
            {
                return Self::from_existing_installation(&manifest_path, bin_dir, env_root).await;
            }

            tokio::fs::File::create(&manifest_path)
                .await
                .into_diagnostic()
                .wrap_err_with(|| format!("Couldn't create file {}", manifest_path.display()))?;
        }

        Self::from_path(&manifest_path)
    }

    async fn from_existing_installation(
        manifest_path: &Path,
        bin_dir: &BinDir,
        env_root: &EnvRoot,
    ) -> miette::Result<Self> {
        let exposed_binaries: Vec<ExposedData> = bin_dir
            .files()
            .await?
            .into_iter()
            .filter_map(|path| match is_text(&path) {
                Ok(true) => Some(Ok(path)), // Success and is text, continue with path
                Ok(false) => None,          // Success and isn't text, filter out
                Err(e) => Some(Err(e)),     // Failure, continue with error
            })
            .map(|result| result.and_then(Self::exposed_data_from_binary_path))
            .collect::<miette::Result<_>>()?;

        let parsed_manifest = ParsedManifest::from(exposed_binaries);
        let toml = toml_edit::ser::to_string(&parsed_manifest).into_diagnostic()?;

        Self::from_str(manifest_path, &toml)
    }

    fn exposed_data_from_binary_path(path: PathBuf) -> miette::Result<ExposedData> {
        let exposed = path
            .file_stem()
            .and_then(OsStr::to_str)
            .ok_or_else(|| miette::miette!("Could not get file stem of {}", path.display()))
            .and_then(ExposedKey::from_str)?;
        let binary_path = Self::extract_bin_from_script(&path)?;

        let binary = binary_path
            .file_stem()
            .and_then(OsStr::to_str)
            .map(String::from)
            .ok_or_else(|| miette::miette!("Could not get file stem of {}", path.display()))?;
        let env_path = binary_path
            .parent()
            .ok_or_else(|| {
                miette::miette!("binary_path '{}' has no parent", binary_path.display())
            })?
            .parent()
            .ok_or_else(|| {
                miette::miette!(
                    "binary_path's parent '{}' has no parent",
                    binary_path.display()
                )
            })?;
        let env = env_path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| {
                miette::miette!(
                    "binary_path's grandparent '{}' has no file name",
                    binary_path.display()
                )
            })
            .and_then(|env| EnvironmentName::from_str(env).into_diagnostic())?;

        let conda_meta = env_path.join("conda-meta");

        let (platform, channel, package) = Self::package_from_conda_meta(&conda_meta, &binary)?;

        Ok(ExposedData {
            env,
            platform,
            channel,
            package,
            binary,
            exposed,
        })
    }

    fn package_from_conda_meta(
        conda_meta: &Path,
        binary: &str,
    ) -> miette::Result<(Option<Platform>, PrioritizedChannel, PackageName)> {
        for entry in std::fs::read_dir(conda_meta)
            .into_diagnostic()
            .wrap_err_with(|| format!("Couldn't read directory {}", conda_meta.display()))?
        {
            let path = entry
                .into_diagnostic()
                .wrap_err_with(|| {
                    format!("Couldn't read file from directory {}", conda_meta.display())
                })?
                .path();

            // Check if the entry is a file and has a .json extension
            if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                let content = std::fs::read_to_string(&path).into_diagnostic()?;
                let json: serde_json::Value = serde_json::from_str(&content)
                    .into_diagnostic()
                    .wrap_err_with(|| format!("Could not parse json from {}", path.display()))?;

                // Check if the JSON contains the specified structure
                if let Some(paths) = json.pointer("/paths_data/paths") {
                    if let Some(array) = paths.as_array() {
                        for item in array {
                            if let Some(path_value) = item.get("_path") {
                                if let Some(path_str) = path_value.as_str() {
                                    if path_str == format!("bin/{binary}") {
                                        let platform = json
                                            .pointer("/subdir")
                                            .map(|p| p.to_string())
                                            .map(|p| Platform::from_str(&p))
                                            .map(|p| match p {
                                                Ok(Platform::NoArch) => None,
                                                Ok(platform) if platform == Platform::current() => {
                                                    None
                                                }
                                                Err(_) => None,
                                                Ok(p) => Some(p),
                                            })
                                            .ok_or_else(|| {
                                                miette!(
                                                    "Could not find platform in {}",
                                                    conda_meta.display()
                                                )
                                            })?;
                                        let channel = json
                                            .pointer("/channel")
                                            .map(|c| c.to_string())
                                            .ok_or_else(|| {
                                                miette!(
                                                    "Could not find channel in {}",
                                                    conda_meta.display()
                                                )
                                            })
                                            .and_then(|c| {
                                                NamedChannelOrUrl::from_str(&c).into_diagnostic()
                                            })
                                            .map(PrioritizedChannel::from)?;
                                        let package = json
                                            .pointer("/name")
                                            .map(|p| p.to_string())
                                            .ok_or_else(|| {
                                                miette!(
                                                    "Could not find package name in {}",
                                                    conda_meta.display()
                                                )
                                            })
                                            .and_then(|p| {
                                                PackageName::from_str(&p).into_diagnostic()
                                            })?;
                                        return Ok((platform, channel, package));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        miette::bail!("Could not find {binary} in {}", conda_meta.display())
    }

    fn extract_bin_from_script(script: &Path) -> miette::Result<PathBuf> {
        // Read the script file into a string
        let script_content = std::fs::read_to_string(script)
            .into_diagnostic()
            .wrap_err_with(|| format!("Could not read {}", script.display()))?;

        // Compile the regex pattern
        #[cfg(unix)]
        const PATTERN: &str = r#""([^"]+)" "\$@""#;
        #[cfg(windows)]
        const PATTERN: &str = r#"^"([^"]+)"\s.*\$"#;
        static RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(PATTERN).expect("Failed to compile regex"));

        // Apply the regex to the script content
        if let Some(caps) = RE.captures(&script_content) {
            if let Some(matched) = caps.get(1) {
                return Ok(PathBuf::from(matched.as_str()));
            }
        }

        // Return an error if the binary path could not be extracted
        miette::bail!(
            "Failed to extract binary path from script {}",
            script.display()
        )
    }

    /// Get default dir for the pixi global manifest
    pub(crate) fn manifest_dir() -> miette::Result<PathBuf> {
        home_path()
            .map(|dir| dir.join("manifests"))
            .ok_or_else(|| miette::miette!("Could not get home directory"))
    }

    /// Loads a project from manifest file.
    pub(crate) fn from_path(manifest_path: &Path) -> miette::Result<Self> {
        let manifest = Manifest::from_path(manifest_path)?;
        Ok(Project::from_manifest(manifest))
    }

    /// Merge config with existing config project
    pub(crate) fn with_cli_config<C>(mut self, config: C) -> Self
    where
        C: Into<Config>,
    {
        self.config = self.config.merge_config(config.into());
        self
    }

    /// Returns the environments in this project.
    pub(crate) fn environments(&self) -> IndexMap<EnvironmentName, ParsedEnvironment> {
        self.manifest.parsed.environments()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use fake::{faker::filesystem::zh_tw::FilePath, Fake};

    const SIMPLE_MANIFEST: &str = r#"
        [envs.python]
        channels = ["conda-forge"]
        [envs.python.dependencies]
        python = "3.11.*"
        [envs.python.exposed]
        python = "python"
        "#;

    #[test]
    fn test_project_from_str() {
        let manifest_path: PathBuf = FilePath().fake();

        let project = Project::from_str(&manifest_path, SIMPLE_MANIFEST).unwrap();
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[test]
    fn test_project_from_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(MANIFEST_DEFAULT_NAME);

        // Create and write global manifest
        let mut file = std::fs::File::create(&manifest_path).unwrap();
        file.write_all(SIMPLE_MANIFEST.as_bytes()).unwrap();
        let project = Project::from_path(&manifest_path).unwrap();

        // Canonicalize both paths
        let canonical_root = project.root.canonicalize().unwrap();
        let canonical_manifest_parent = manifest_path.parent().unwrap().canonicalize().unwrap();

        assert_eq!(canonical_root, canonical_manifest_parent);
    }

    #[test]
    fn test_project_from_manifest() {
        let manifest_path: PathBuf = FilePath().fake();

        let manifest = Manifest::from_str(&manifest_path, SIMPLE_MANIFEST).unwrap();
        let project = Project::from_manifest(manifest);
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[test]
    fn test_project_manifest_dir() {
        Project::manifest_dir().unwrap();
    }
}
