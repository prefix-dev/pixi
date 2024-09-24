use super::{BinDir, EnvRoot};
use crate::{
    global::{common::is_text, find_executables, EnvDir},
    prefix::Prefix,
};
pub(crate) use environment::EnvironmentName;
use fs::tokio as tokio_fs;
use fs_err as fs;
use indexmap::IndexMap;
pub(crate) use manifest::{Manifest, Mapping};
use miette::{Context, IntoDiagnostic};
use once_cell::sync::Lazy;
pub(crate) use parsed_manifest::ExposedName;
pub(crate) use parsed_manifest::ParsedEnvironment;
use parsed_manifest::ParsedManifest;
use pixi_config::{home_path, Config};
use pixi_consts::consts;
use pixi_manifest::PrioritizedChannel;
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform, PrefixRecord};
use regex::Regex;
use std::{
    ffi::OsStr,
    fmt::{Debug, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

mod environment;
mod manifest;
mod parsed_manifest;

pub(crate) const MANIFEST_DEFAULT_NAME: &str = "pixi-global.toml";
pub(crate) const MANIFESTS_DIR: &str = "manifests";

/// The pixi global project, this main struct to interact with the pixi global
/// project. This struct holds the `Manifest` and has functions to modify
/// or request information from it. This allows in the future to have multiple
/// manifests linked to a pixi global project.
#[derive(Clone)]
pub struct Project {
    /// Root folder of the project
    root: PathBuf,
    /// The manifest for the project
    pub(crate) manifest: Manifest,
    /// The global configuration as loaded from the config file(s)
    config: Config,
    /// Root directory of the global environments
    pub(crate) env_root: EnvRoot,
    /// Binary directory
    pub(crate) bin_dir: BinDir,
}

impl Debug for Project {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Global Project")
            .field("root", &self.root)
            .field("manifest", &self.manifest)
            .finish()
    }
}

/// Intermediate struct to store all the binaries that are exposed.
#[derive(Debug)]
struct ExposedData {
    env_name: EnvironmentName,
    platform: Option<Platform>,
    channel: PrioritizedChannel,
    package: PackageName,
    exposed: ExposedName,
    executable_name: String,
}

impl ExposedData {
    /// Constructs an `ExposedData` instance from a exposed script path.
    ///
    /// This function extracts metadata from the exposed script path, including the
    /// environment name, platform, channel, and package information, by reading
    /// the associated `conda-meta` directory.
    pub async fn from_exposed_path(path: &Path, env_root: &EnvRoot) -> miette::Result<Self> {
        let exposed = path
            .file_stem()
            .and_then(OsStr::to_str)
            .ok_or_else(|| miette::miette!("Could not get file stem of {}", path.display()))
            .and_then(ExposedName::from_str)?;
        let executable_path = extract_executable_from_script(path)?;

        let executable = executable_path
            .file_stem()
            .and_then(OsStr::to_str)
            .map(String::from)
            .ok_or_else(|| miette::miette!("Could not get file stem of {}", path.display()))?;

        let env_path = determine_env_path(&executable_path, env_root.path())?;
        let env_name = env_path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| {
                miette::miette!(
                    "executable path's grandparent '{}' has no file name",
                    executable_path.display()
                )
            })
            .and_then(|env| EnvironmentName::from_str(env).into_diagnostic())?;

        let conda_meta = env_path.join(consts::CONDA_META_DIR);
        let bin_env_dir = EnvDir::from_env_root(env_root.clone(), env_name.clone()).await?;
        let prefix = Prefix::new(bin_env_dir.path());

        let (platform, channel, package) =
            package_from_conda_meta(&conda_meta, &executable, &prefix).await?;

        Ok(ExposedData {
            env_name,
            platform,
            channel,
            package,
            executable_name: executable,
            exposed,
        })
    }
}

/// Extracts the executable path from a script file.
///
/// This function reads the content of the script file and attempts to extract
/// the path of the executable it references. It is used to determine
/// the actual binary path from a wrapper script.
fn extract_executable_from_script(script: &Path) -> miette::Result<PathBuf> {
    // Read the script file into a string
    let script_content = fs::read_to_string(script)
        .into_diagnostic()
        .wrap_err_with(|| format!("Could not read {}", script.display()))?;

    // Compile the regex pattern
    #[cfg(unix)]
    const PATTERN: &str = r#""([^"]+)" "\$@""#;
    #[cfg(windows)]
    const PATTERN: &str = r#"@"([^"]+)" %/*"#;
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(PATTERN).expect("Failed to compile regex"));

    // Apply the regex to the script content
    if let Some(caps) = RE.captures(&script_content) {
        if let Some(matched) = caps.get(1) {
            return Ok(PathBuf::from(matched.as_str()));
        }
    }

    // Return an error if the executable path could not be extracted
    miette::bail!(
        "Failed to extract executable path from script {}",
        script.display()
    )
}

fn determine_env_path(executable_path: &Path, env_root: &Path) -> miette::Result<PathBuf> {
    let mut current_path = executable_path;

    while let Some(parent) = current_path.parent() {
        if parent == env_root {
            return Ok(current_path.to_owned());
        }
        current_path = parent;
    }

    miette::bail!(
        "Could not determine environment path: no parent of '{}' has '{}' as its direct parent",
        executable_path.display(),
        env_root.display()
    )
}

/// Extracts package metadata from the `conda-meta` directory for a given executable.
///
/// This function reads the `conda-meta` directory to find the package metadata
/// associated with the specified executable. It returns the platform, channel, and
/// package name of the executable.
async fn package_from_conda_meta(
    conda_meta: &Path,
    executable: &str,
    prefix: &Prefix,
) -> miette::Result<(Option<Platform>, PrioritizedChannel, PackageName)> {
    let mut read_dir = tokio_fs::read_dir(conda_meta).await.into_diagnostic()?;

    while let Some(entry) = read_dir.next_entry().await.into_diagnostic()? {
        let path = entry.path();
        // Check if the entry is a file and has a .json extension
        if path.is_file() && path.extension().and_then(OsStr::to_str) == Some("json") {
            let prefix_record = PrefixRecord::from_path(&path)
                .into_diagnostic()
                .wrap_err_with(|| format!("Could not parse json from {}", path.display()))?;

            if find_executables(prefix, &prefix_record)
                .iter()
                .any(|exe_path| exe_path.file_stem().and_then(OsStr::to_str) == Some(executable))
            {
                let platform = match Platform::from_str(
                    &prefix_record.repodata_record.package_record.subdir,
                ) {
                    Ok(Platform::NoArch) => None,
                    Ok(platform) if platform == Platform::current() => None,
                    Err(_) => None,
                    Ok(p) => Some(p),
                };

                let channel: PrioritizedChannel =
                    NamedChannelOrUrl::from_str(&prefix_record.repodata_record.channel)
                        .into_diagnostic()?
                        .into();

                let name = prefix_record.repodata_record.package_record.name;

                return Ok((platform, channel, name));
            }
        }
    }

    miette::bail!("Could not find {executable} in {}", conda_meta.display())
}

impl Project {
    /// Constructs a new instance from an internal manifest representation
    pub(crate) fn from_manifest(manifest: Manifest, env_root: EnvRoot, bin_dir: BinDir) -> Self {
        let root = manifest
            .path
            .parent()
            .expect("manifest path should always have a parent")
            .to_owned();

        let config = Config::load(&root);

        Self {
            root,
            manifest,
            config,
            env_root,
            bin_dir,
        }
    }

    /// Constructs a project from a manifest.
    pub(crate) fn from_str(
        manifest_path: &Path,
        content: &str,
        env_root: EnvRoot,
        bin_dir: BinDir,
    ) -> miette::Result<Self> {
        let manifest = Manifest::from_str(manifest_path, content)?;
        Ok(Self::from_manifest(manifest, env_root, bin_dir))
    }

    /// Discovers the project manifest file in path at
    /// `~/.pixi/manifests/pixi-global.toml`. If the manifest doesn't exist
    /// yet, and the function will try to create one from the existing
    /// installation. If that one fails, an empty one will be created.
    pub(crate) async fn discover_or_create(assume_yes: bool) -> miette::Result<Self> {
        let manifest_dir = Self::manifest_dir()?;
        let manifest_path = manifest_dir.join(MANIFEST_DEFAULT_NAME);
        // Prompt user if the manifest is empty and the user wants to create one

        let bin_dir = BinDir::from_env().await?;
        let env_root = EnvRoot::from_env().await?;

        if !manifest_path.exists() {
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
                return Self::try_from_existing_installation(&manifest_path, env_root, bin_dir)
                    .await
                    .wrap_err_with(|| {
                        "Failed to create global manifest from existing installation"
                    });
            }

            tokio_fs::create_dir_all(&manifest_dir)
                .await
                .into_diagnostic()
                .wrap_err_with(|| {
                    format!("Couldn't create directory {}", manifest_dir.display())
                })?;

            tokio_fs::File::create(&manifest_path)
                .await
                .into_diagnostic()
                .wrap_err_with(|| format!("Could not create file {}", manifest_path.display()))?;
        }

        Self::from_path(&manifest_path, env_root, bin_dir)
    }

    async fn try_from_existing_installation(
        manifest_path: &Path,
        env_root: EnvRoot,
        bin_dir: BinDir,
    ) -> miette::Result<Self> {
        let futures = bin_dir
            .files()
            .await?
            .into_iter()
            .filter_map(|path| match is_text(&path) {
                Ok(true) => Some(Ok(path)), // Success and is text, continue with path
                Ok(false) => None,          // Success and isn't text, filter out
                Err(e) => Some(Err(e)),     // Failure, continue with error
            })
            .map(|result| async {
                match result {
                    Ok(path) => ExposedData::from_exposed_path(&path, &env_root).await,
                    Err(e) => Err(e),
                }
            });

        let exposed_binaries: Vec<ExposedData> = futures::future::try_join_all(futures).await.wrap_err_with(|| {
            "Failed to extract exposed binaries from existing installation please clean up your installation."
        })?;
        let parsed_manifest = ParsedManifest::from(exposed_binaries);
        let toml = toml_edit::ser::to_string_pretty(&parsed_manifest).into_diagnostic()?;
        tokio_fs::write(&manifest_path, &toml)
            .await
            .into_diagnostic()?;
        Self::from_str(manifest_path, &toml, env_root, bin_dir)
    }

    /// Get default dir for the pixi global manifest
    pub(crate) fn manifest_dir() -> miette::Result<PathBuf> {
        home_path()
            .map(|dir| dir.join(MANIFESTS_DIR))
            .ok_or_else(|| miette::miette!("Could not get home directory"))
    }

    /// Loads a project from manifest file.
    pub(crate) fn from_path(
        manifest_path: &Path,
        env_root: EnvRoot,
        bin_dir: BinDir,
    ) -> miette::Result<Self> {
        let manifest = Manifest::from_path(manifest_path)?;
        Ok(Project::from_manifest(manifest, env_root, bin_dir))
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
    pub(crate) fn environments(&self) -> &IndexMap<EnvironmentName, ParsedEnvironment> {
        &self.manifest.parsed.envs
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use fake::{faker::filesystem::zh_tw::FilePath, Fake};

    use super::*;

    const SIMPLE_MANIFEST: &str = r#"
        [envs.python]
        channels = ["dummy-channel"]
        [envs.python.dependencies]
        dummy = "3.11.*"
        [envs.python.exposed]
        dummy = "dummy"
        "#;

    #[tokio::test]
    async fn test_project_from_str() {
        let manifest_path: PathBuf = FilePath().fake();
        let env_root = EnvRoot::from_env().await.unwrap();
        let bin_dir = BinDir::from_env().await.unwrap();

        let project =
            Project::from_str(&manifest_path, SIMPLE_MANIFEST, env_root, bin_dir).unwrap();
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[tokio::test]
    async fn test_project_from_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(MANIFEST_DEFAULT_NAME);

        let env_root = EnvRoot::from_env().await.unwrap();
        let bin_dir = BinDir::from_env().await.unwrap();

        // Create and write global manifest
        let mut file = fs::File::create(&manifest_path).unwrap();
        file.write_all(SIMPLE_MANIFEST.as_bytes()).unwrap();
        let project = Project::from_path(&manifest_path, env_root, bin_dir).unwrap();

        // Canonicalize both paths
        let canonical_root = project.root.canonicalize().unwrap();
        let canonical_manifest_parent = manifest_path.parent().unwrap().canonicalize().unwrap();

        assert_eq!(canonical_root, canonical_manifest_parent);
    }

    #[tokio::test]
    async fn test_project_from_manifest() {
        let manifest_path: PathBuf = FilePath().fake();

        let env_root = EnvRoot::from_env().await.unwrap();
        let bin_dir = BinDir::from_env().await.unwrap();

        let manifest = Manifest::from_str(&manifest_path, SIMPLE_MANIFEST).unwrap();
        let project = Project::from_manifest(manifest, env_root, bin_dir);
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[test]
    fn test_project_manifest_dir() {
        Project::manifest_dir().unwrap();
    }
}
