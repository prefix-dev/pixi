use super::{expose_executables, BinDir, EnvRoot};
use crate::global::install::local_environment_matches_spec;
use crate::repodata::Repodata;
use crate::rlimit::try_increase_rlimit_to_sensible;
use crate::{
    global::{common::is_text, find_executables, EnvDir},
    prefix::Prefix,
};
pub(crate) use environment::EnvironmentName;
use fs::tokio as tokio_fs;
use fs_err as fs;
use indexmap::IndexMap;
use itertools::Itertools;
pub(crate) use manifest::{Manifest, Mapping};
use miette::{miette, Context, IntoDiagnostic};
use once_cell::sync::Lazy;
pub(crate) use parsed_manifest::ExposedName;
pub(crate) use parsed_manifest::ParsedEnvironment;
use parsed_manifest::ParsedManifest;
use pixi_config::{default_channel_config, home_path, Config};
use pixi_consts::consts;
use pixi_manifest::PrioritizedChannel;
use pixi_progress::{await_in_progress, global_multi_progress, wrap_in_progress};
use pixi_utils::executable_from_path;
use pixi_utils::reqwest::build_reqwest_clients;
use rattler::install::{DefaultProgressFormatter, IndicatifReporter, Installer};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, NamedChannelOrUrl, PackageName, Platform, PrefixRecord};
use rattler_repodata_gateway::Gateway;
use rattler_solve::resolvo::Solver;
use rattler_solve::{SolverImpl, SolverTask};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use regex::Regex;
use reqwest_middleware::ClientWithMiddleware;
use std::sync::OnceLock;
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
    /// Reqwest client shared for this project.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    client: OnceLock<(reqwest::Client, ClientWithMiddleware)>,
    /// The repodata gateway to use for answering queries about repodata.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    repodata_gateway: OnceLock<Gateway>,
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
        let exposed = ExposedName::from_str(executable_from_path(path).as_str())?;
        let executable_path = extract_executable_from_script(path).await?;

        let executable = executable_from_path(&executable_path);
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
        let env_dir = EnvDir::from_env_root(env_root.clone(), env_name.clone()).await?;
        let prefix = Prefix::new(env_dir.path());

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
async fn extract_executable_from_script(script: &Path) -> miette::Result<PathBuf> {
    // Read the script file into a string
    let script_content = tokio_fs::read_to_string(script).await.into_diagnostic()?;

    // Compile the regex pattern
    #[cfg(unix)]
    const PATTERN: &str = r#""([^"]+)" "\$@""#;
    // The pattern includes `"?` to also find old pixi global installations.
    #[cfg(windows)]
    const PATTERN: &str = r#"@"?([^"]+)"? %/*"#;
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(PATTERN).expect("Failed to compile regex"));

    // Apply the regex to the script content
    if let Some(caps) = RE.captures(&script_content) {
        if let Some(matched) = caps.get(1) {
            return Ok(PathBuf::from(matched.as_str()));
        }
    }
    tracing::debug!(
        "Failed to extract executable path from script {}",
        script_content
    );

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
                .any(|exe_path| executable_from_path(exe_path) == executable)
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

        let client = OnceLock::new();
        let repodata_gateway = OnceLock::new();
        Self {
            root,
            manifest,
            config,
            env_root,
            bin_dir,
            client,
            repodata_gateway,
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
                .into_diagnostic()?;

            tokio_fs::File::create(&manifest_path)
                .await
                .into_diagnostic()?;
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

    /// Return the environment with the given name.
    pub(crate) fn environment(&self, name: &EnvironmentName) -> Option<&ParsedEnvironment> {
        self.manifest.parsed.envs.get(name)
    }

    /// Create an authenticated reqwest client for this project
    /// use authentication from `rattler_networking`
    pub fn authenticated_client(&self) -> &ClientWithMiddleware {
        &self.client_and_authenticated_client().1
    }

    fn client_and_authenticated_client(&self) -> &(reqwest::Client, ClientWithMiddleware) {
        self.client
            .get_or_init(|| build_reqwest_clients(Some(&self.config)))
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) async fn install_environment(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<()> {
        let environment = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment '{}' not found", env_name))?;
        let channels = environment
            .sorted_named_channels()
            .into_iter()
            .map(|channel| {
                channel
                    .clone()
                    .into_channel(self.config.global_channel_config())
            })
            .collect::<Result<Vec<_>, _>>()
            .into_diagnostic()?;

        let platform = environment.platform.unwrap_or_else(Platform::current);



        let match_specs = environment
            .dependencies
            .clone()
            .into_iter()
            .map(|(name, spec)| {
                if let Some(nameless_spec) = spec
                    .clone()
                    .try_into_nameless_match_spec(self.config().global_channel_config())
                    .into_diagnostic()?
                {
                    Ok(MatchSpec::from_nameless(nameless_spec, Some(name.clone())))
                } else {
                    Err(miette!("Could not convert {spec:?} to nameless match spec."))
                }
            })
            .collect::<miette::Result<Vec<MatchSpec>>>()?;


        let repodata = await_in_progress("querying repodata ", |_| async {
            self.repodata_gateway()
                .query(channels, [platform, Platform::NoArch], match_specs.clone())
                .recursive(true)
                .await
                .into_diagnostic()
        })
        .await?;

        // Determine virtual packages of the current platform
        let virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::default())
            .into_diagnostic()
            .context("failed to determine virtual packages")?
            .iter()
            .cloned()
            .map(GenericVirtualPackage::from)
            .collect();

        // Solve the environment

        let solved_records = tokio::task::spawn_blocking(move || {
            wrap_in_progress("solving environment", move || {
                Solver.solve(SolverTask {
                    specs: match_specs,
                    virtual_packages,
                    ..SolverTask::from_iter(&repodata)
                })
            })
            .into_diagnostic()
            .context("failed to solve environment")
        })
        .await
        .into_diagnostic()??;

        try_increase_rlimit_to_sensible();

        // Install the environment
        let package_cache = PackageCache::new(pixi_config::get_cache_dir()?.join("pkgs"));
        let prefix = Prefix::new(
            EnvDir::from_env_root(self.env_root.clone(), env_name.clone())
                .await?
                .path(),
        );
        await_in_progress("creating virtual environment", |pb| {
            Installer::new()
                .with_download_client(self.authenticated_client().clone())
                .with_io_concurrency_limit(100)
                .with_execute_link_scripts(false)
                .with_package_cache(package_cache)
                .with_target_platform(platform)
                .with_reporter(
                    IndicatifReporter::builder()
                        .with_multi_progress(global_multi_progress())
                        .with_placement(rattler::install::Placement::After(pb))
                        .with_formatter(DefaultProgressFormatter::default().with_prefix("  "))
                        .clear_when_done(true)
                        .finish(),
                )
                .install(prefix.root(), solved_records)
        })
        .await
        .into_diagnostic()?;

        Ok(())
    }

    // Syncs the manifest with the local environments
    // Returns true if the global installation had to be updated
    pub(crate) async fn sync(&self) -> Result<bool, miette::Error> {
        let mut updated_env = false;

        // Prune environments that are not listed
        updated_env |= !self
            .env_root
            .prune(self.environments().keys().cloned())
            .await?
            .is_empty();

        // Remove binaries that are not listed as exposed
        let exposed_paths = self
            .environments()
            .values()
            .flat_map(|environment| {
                environment
                    .exposed
                    .keys()
                    .map(|e| self.bin_dir.executable_script_path(e))
            })
            .collect_vec();
        for file in self.bin_dir.files().await? {
            let file_name = executable_from_path(&file);
            if !exposed_paths.contains(&file) && file_name != "pixi" {
                tokio_fs::remove_file(&file).await.into_diagnostic()?;
                updated_env = true;
                eprintln!(
                    "{}Remove executable '{file_name}'.",
                    console::style(console::Emoji("✔ ", "")).green()
                );
            }
        }

        for (env_name, _parsed_environment) in self.environments() {
            self.sync_environment(env_name).await?;
        }

        Ok(updated_env)
    }

    /// Syncs the parsed environment with the installation.
    /// Returns true if the environment had to be updated.
    pub(crate) async fn sync_environment(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<bool> {
        let mut updated_env = false;
        let environment = self.environment(env_name).ok_or(miette::miette!(
            "Environment {} not found.",
            env_name.to_string()
        ))?;

        let specs = environment
            .dependencies
            .clone()
            .into_iter()
            .map(|(name, spec)| {
                let match_spec = MatchSpec::from_nameless(
                    spec.clone()
                        .try_into_nameless_match_spec(&default_channel_config())
                        .into_diagnostic()?
                        .ok_or_else(|| {
                            miette::miette!("Could not convert {spec:?} to nameless match spec.")
                        })?,
                    Some(name.clone()),
                );
                Ok((name, match_spec))
            })
            .collect::<Result<IndexMap<PackageName, MatchSpec>, miette::Report>>()?;

        let env_dir = EnvDir::from_env_root(self.env_root.clone(), env_name.clone()).await?;
        let prefix = Prefix::new(env_dir.path());

        let repodata_records = prefix
            .find_installed_packages(Some(50))
            .await?
            .into_iter()
            .map(|r| r.repodata_record)
            .collect_vec();

        let install_env =
            !local_environment_matches_spec(repodata_records, &specs, environment.platform);

        updated_env |= install_env;

        if install_env {
            self.install_environment(env_name).await?;
        }

        updated_env |= expose_executables(env_name, environment, &prefix, &self.bin_dir).await?;

        Ok(updated_env)
    }
}

impl Repodata for Project {
    /// Returns the [`Gateway`] used by this project.
    fn repodata_gateway(&self) -> &Gateway {
        self.repodata_gateway.get_or_init(|| {
            Self::repodata_gateway_init(
                self.authenticated_client().clone(),
                self.config().clone().into(),
            )
        })
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

    #[cfg(windows)]
    #[tokio::test]
    async fn test_extract_executable_from_script_windows() {
        let script_without_quote = r#"
@SET "PATH=C:\Users\USER\.pixi/envs\hyperfine\bin:%PATH%"
@SET "CONDA_PREFIX=C:\Users\USER\.pixi/envs\hyperfine"
@C:\Users\USER\.pixi/envs\hyperfine\bin/hyperfine.exe %*
"#;
        let script_path = Path::new("hyperfine.bat");
        let tempdir = tempfile::tempdir().unwrap();
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script_without_quote).unwrap();
        let executable_path = extract_executable_from_script(&script_path).await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("C:\\Users\\USER\\.pixi/envs\\hyperfine\\bin/hyperfine.exe")
        );

        let script_with_quote = r#"
@SET "PATH=C:\Users\USER\.pixi/envs\python\bin;%PATH%"
@SET "CONDA_PREFIX=C:\Users\USER\.pixi/envs\python"
@"C:\Users\USER\.pixi\envs\python\Scripts/pydoc.exe" %*
"#;
        let script_path = Path::new("pydoc.bat");
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script_with_quote).unwrap();
        let executable_path = extract_executable_from_script(&script_path).await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("C:\\Users\\USER\\.pixi\\envs\\python\\Scripts/pydoc.exe")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_extract_executable_from_script_unix() {
        let script = r#"#!/bin/sh
export PATH="/home/user/.pixi/envs/nushell/bin:${PATH}"
export CONDA_PREFIX="/home/user/.pixi/envs/nushell"
"/home/user/.pixi/envs/nushell/bin/nu" "$@"
"#;
        let script_path = Path::new("nu");
        let tempdir = tempfile::tempdir().unwrap();
        let script_path = tempdir.path().join(script_path);
        fs::write(&script_path, script).unwrap();
        let executable_path = extract_executable_from_script(&script_path).await.unwrap();
        assert_eq!(
            executable_path,
            Path::new("/home/user/.pixi/envs/nushell/bin/nu")
        );
    }
}
