use std::{
    collections::{BTreeSet as Set, HashMap, HashSet},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    sync::{LazyLock, Mutex},
};

use clap::{ArgAction, Parser};
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, miette};
use pixi_consts::consts;
use rattler_conda_types::{
    ChannelConfig, NamedChannelOrUrl, Platform, Version, VersionBumpType, VersionSpec,
    version_spec::{EqualityOperator, LogicalOperator, RangeOperator},
};
use rattler_config::config::ConfigBase;
// Bring the shared `Config` trait methods (merge_config, keys, ...) of the
// rattler_config types into scope under a non-clashing name.
use rattler_config::config::Config as RattlerConfig;
use rattler_config::config::{CommonConfig, MergeError};
use rattler_networking::s3_middleware;
use rattler_repodata_gateway::{Gateway, GatewayBuilder, SourceConfig};
use reqwest::{NoProxy, Proxy};
use serde::{Deserialize, Serialize, de::IntoDeserializer};
use url::Url;

/// Controls which root certificates to use for TLS connections.
///
/// This is the shared `rattler_config` type: the legacy pixi spellings
/// `"native"` and `"all"` are accepted as deserialization aliases of
/// [`TlsRootCerts::System`]. pixi still emits a deprecation warning when a
/// config file or the CLI uses one of the legacy spellings (see
/// [`Config::from_toml`]).
///
/// Note: this setting only has an effect when pixi is built with the `rustls`
/// feature. When built with `native-tls`, system certificates are always used
/// regardless of this setting. `SSL_CERT_FILE` / `SSL_CERT_DIR` (when set and
/// valid) always take precedence over this setting.
pub use rattler_config::config::tls::TlsRootCerts;

/// Clap value parser for `--tls-root-certs` that keeps the deprecation
/// warning for the legacy `"native"` / `"all"` spellings, which the shared
/// [`TlsRootCerts`] enum silently accepts as aliases of `System`.
fn parse_tls_root_certs_cli(
    raw: &str,
) -> Result<TlsRootCerts, rattler_config::config::tls::ParseTlsRootCertsError> {
    let parsed = raw.parse()?;
    warn_deprecated_tls_root_certs(raw, None);
    Ok(parsed)
}

pub fn default_channel_config() -> ChannelConfig {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    ChannelConfig::default_with_root_dir(cwd)
}

/// Determines the default author based on the default git author. Both the name
/// and the email address of the author are returned.
pub fn get_default_author() -> Option<(String, String)> {
    let rv = Command::new("git")
        .arg("config")
        .arg("--get-regexp")
        .arg("^user.(name|email)$")
        .stdout(Stdio::piped())
        .output()
        .ok()?;

    let mut name = None;
    let mut email = None;

    for line in std::str::from_utf8(&rv.stdout).ok()?.lines() {
        match line.split_once(' ') {
            Some(("user.email", value)) => {
                email = Some(value.to_string());
            }
            Some(("user.name", value)) => {
                name = Some(value.to_string());
            }
            _ => {}
        }
    }

    Some((name?, email.unwrap_or_else(|| "".into())))
}

// detect proxy env vars like curl: https://curl.se/docs/manpage.html
static ENV_HTTP_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["http_proxy", "all_proxy", "ALL_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static ENV_HTTPS_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static ENV_NO_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["no_proxy", "NO_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static USE_PROXY_FROM_ENV: LazyLock<bool> =
    LazyLock::new(|| (*ENV_HTTPS_PROXY).is_some() || (*ENV_HTTP_PROXY).is_some());

static NETFS_REDIRECT_WARNED: LazyLock<Mutex<HashSet<CacheKind>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Lazily-loaded global (system + user) cache config.
///
/// Used by [`get_cache_dir`] to resolve the cache *root* for callers that
/// don't have a [`Config`] handy. Per-kind cache directories that should
/// honor workspace-level `[cache.*]` overrides must be resolved through
/// [`Config::cache_dir_for`] instead.
static GLOBAL_CACHE_CONFIG: LazyLock<CacheConfig> =
    LazyLock::new(|| Config::load_global().extensions.cache.clone());

/// Describes where the system + user-level config layer comes from. Built from
/// [`ConfigSourceCli`] (which mirrors `--no-config` / `--config-file`) and
/// passed into [`Config::load_global_with`] and [`Config::load_with`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GlobalConfigSource {
    /// Search the system and user-level paths (the default).
    #[default]
    Search,
    /// Skip every system and user-level config file.
    None,
    /// Load only this file as the global layer.
    File(PathBuf),
}

/// CLI flags that select where the global (system + user-level) config layer is
/// read from. Flattened into each subcommand's `Args` so `--no-config` /
/// `--config-file` are available per command.
#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigSourceCli {
    /// Don't read system or user-level configuration files. Project-local
    /// `<project>/.pixi/config.toml` is still loaded.
    #[arg(
        long,
        env = "PIXI_NO_CONFIG",
        value_parser = clap::builder::BoolishValueParser::new(),
        default_value_t = false,
        conflicts_with = "config_file",
        help_heading = consts::CLAP_CONFIG_OPTIONS
    )]
    pub no_config: bool,

    /// Load configuration from this file instead of searching system and
    /// user-level paths. Project-local `<project>/.pixi/config.toml` is still
    /// merged on top.
    #[arg(
        long,
        value_name = "PATH",
        env = "PIXI_CONFIG_FILE",
        conflicts_with = "no_config",
        help_heading = consts::CLAP_CONFIG_OPTIONS
    )]
    pub config_file: Option<PathBuf>,
}

impl ConfigSourceCli {
    /// Translate the flags into a [`GlobalConfigSource`].
    pub fn source(&self) -> GlobalConfigSource {
        if self.no_config {
            GlobalConfigSource::None
        } else if let Some(path) = &self.config_file {
            GlobalConfigSource::File(path.clone())
        } else {
            GlobalConfigSource::Search
        }
    }
}

/// Get pixi home directory, default to `$HOME/.pixi`
///
/// It may be overridden by the `PIXI_HOME` environment variable.
///
/// # Returns
///
/// The pixi home directory
pub fn pixi_home() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PIXI_HOME") {
        Some(PathBuf::from(path))
    } else {
        dirs::home_dir().map(|path| path.join(consts::PIXI_DIR))
    }
}

// TODO(tim): I think we should move this to another crate, dont know if global
// config is really correct
/// Returns the default cache directory.
/// Most important is the `PIXI_CACHE_DIR` environment variable.
/// - If that is not set, the `RATTLER_CACHE_DIR` environment variable is used.
/// - If that is not set, `XDG_CACHE_HOME/pixi` is used when the directory
///   exists.
/// - If that is not set, the default cache directory of
///   [`rattler::default_cache_dir`] is used.
pub fn get_cache_dir() -> miette::Result<PathBuf> {
    resolve_cache_root(&GLOBAL_CACHE_CONFIG)
        .map(|(path, _)| path)
        .ok_or_else(|| miette::miette!("could not determine default cache directory"))
}

/// How the cache directory was resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheDirSource {
    /// User explicitly pinned via `PIXI_CACHE_DIR`, `RATTLER_CACHE_DIR`, or
    /// `[cache.root]`.
    UserPinned,
    /// XDG default / rattler default.
    Default,
}

/// Returns `true` when `path` (or its first existing ancestor) lives on a
/// network-backed filesystem.
pub fn is_network_filesystem(path: &Path) -> bool {
    if std::env::var_os("PIXI_DISABLE_NETFS_REDIRECT").is_some() {
        return false;
    }
    if std::env::var_os("PIXI_FORCE_NETFS_REDIRECT").is_some() {
        return true;
    }
    detect_network_filesystem(path).unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn detect_network_filesystem(path: &Path) -> Option<bool> {
    use nix::sys::statfs::{
        AUTOFS_SUPER_MAGIC, FUSE_SUPER_MAGIC, FsType, NFS_SUPER_MAGIC, SMB_SUPER_MAGIC,
    };
    // Magics not re-exported by `nix` are defined inline below.
    // CIFS: fs/smb/client/cifsfs.h
    let cifs_magic = FsType(0xff53_4d42_u32 as _);
    // BeeGFS (a.k.a. fhgfs): client_module/source/common/Common.h
    let beegfs_magic = FsType(0x1983_0326_u32 as _);
    // Lustre: include/uapi/linux/lustre/lustre_user.h (LL_SUPER_MAGIC)
    let lustre_magic = FsType(0x0bd0_0bd0_u32 as _);
    // GPFS / IBM Spectrum Scale ("GPFS" in ASCII)
    let gpfs_magic = FsType(0x4750_4653_u32 as _);
    // CephFS: include/linux/magic.h (CEPH_SUPER_MAGIC)
    let ceph_magic = FsType(0x00c3_6400_u32 as _);
    let fs = statfs_nearest_existing(path)?.filesystem_type();
    Some(
        fs == NFS_SUPER_MAGIC
            || fs == SMB_SUPER_MAGIC
            || fs == cifs_magic
            || fs == FUSE_SUPER_MAGIC
            || fs == AUTOFS_SUPER_MAGIC
            || fs == beegfs_magic
            || fs == lustre_magic
            || fs == gpfs_magic
            || fs == ceph_magic,
    )
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn detect_network_filesystem(path: &Path) -> Option<bool> {
    let stat = statfs_nearest_existing(path)?;
    Some(matches!(
        stat.filesystem_type_name(),
        "nfs" | "smbfs" | "webdav" | "afpfs" | "macfuse" | "osxfuse"
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn detect_network_filesystem(_path: &Path) -> Option<bool> {
    None
}

#[cfg(unix)]
fn statfs_nearest_existing(path: &Path) -> Option<nix::sys::statfs::Statfs> {
    // Walk upward until we find an ancestor that actually exists
    let mut current = Some(path);
    while let Some(p) = current {
        if p.exists() {
            return nix::sys::statfs::statfs(p).ok();
        }
        current = p.parent();
    }
    None
}

/// Returns a directory suitable for node-local scratch caching on HPC nodes.
///
/// Prefers scheduler-provided tmp dirs (`SLURM_TMPDIR`, `PBS_JOBFS`,
/// `SCRATCH`)
pub fn node_local_scratch_dir() -> PathBuf {
    for var in ["SLURM_TMPDIR", "PBS_JOBFS", "SCRATCH", "TMPDIR"] {
        if let Some(dir) = std::env::var_os(var) {
            let path = PathBuf::from(dir);
            if !detect_network_filesystem(&path).unwrap_or(false) {
                return path;
            }
        }
    }
    std::env::temp_dir()
}

/// Identifies a specific pixi cache directory.
///
///
/// [`CacheKind::prefers_shared`] encodes this preference and is consulted by
/// the auto-redirect logic when the resolved cache root is on a network
/// filesystem. It can be configured using `[cache.<kind>]` in
/// `config.toml`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum CacheKind {
    /// Conda package cache (`pkgs`).
    CondaPackages,
    /// Sharded / classic repodata cache.
    Repodata,
    /// uv wheel cache.
    PypiWheels,
    /// conda → PyPI name-mapping cache.
    PypiMapping,
    /// Cached `pixi exec` environments.
    ExecEnvironments,
    /// Cached build-tool environments.
    BuildToolEnvironments,
    /// Detached environments root (when `detached-environments = true`).
    DetachedEnvironments,
}

impl CacheKind {
    /// The directory name (relative to the cache root) used for this kind.
    pub fn subdir(self) -> &'static str {
        match self {
            CacheKind::CondaPackages => consts::CONDA_PACKAGE_CACHE_DIR,
            CacheKind::Repodata => consts::CONDA_REPODATA_CACHE_DIR,
            CacheKind::PypiWheels => consts::PYPI_CACHE_DIR,
            CacheKind::PypiMapping => consts::CONDA_PYPI_MAPPING_CACHE_DIR,
            CacheKind::ExecEnvironments => consts::CACHED_ENVS_DIR,
            CacheKind::BuildToolEnvironments => consts::CACHED_BUILD_TOOL_ENVS_DIR,
            CacheKind::DetachedEnvironments => consts::ENVIRONMENTS_DIR,
        }
    }

    /// The `[cache.<key>]` TOML field name used to override this kind's path.
    ///
    /// This is distinct from [`Self::subdir`]: the on-disk directory name does
    /// not always match the config key (e.g. the mapping cache lives in
    /// `conda-pypi-mapping` but is configured via `cache.pypi-mapping`).
    pub fn config_key(self) -> &'static str {
        match self {
            CacheKind::CondaPackages => "conda-packages",
            CacheKind::Repodata => "repodata",
            CacheKind::PypiWheels => "pypi-wheels",
            CacheKind::PypiMapping => "pypi-mapping",
            CacheKind::ExecEnvironments => "exec-environments",
            CacheKind::BuildToolEnvironments => "build-tool-environments",
            CacheKind::DetachedEnvironments => "detached-environments",
        }
    }

    /// Whether this cache benefits from being shared across users on a single
    /// (potentially networked) filesystem.
    ///
    pub fn prefers_shared(self) -> bool {
        matches!(self, CacheKind::CondaPackages | CacheKind::PypiWheels)
    }
}

/// Per-cache TOML configuration. Lives under `[cache]` in `config.toml`.
///
/// All path fields are absolute. Setting one bypasses the auto-redirect logic
/// for that kind and uses the configured path verbatim.
#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct CacheConfig {
    /// Override for the cache root. Equivalent to setting `PIXI_CACHE_DIR`,
    /// but persisted in `config.toml`. Per-kind fields below override this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conda_packages: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repodata: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pypi_wheels: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pypi_mapping: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_environments: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_tool_environments: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_environments: Option<PathBuf>,

    /// How to handle a cache that lives on a network filesystem.
    #[serde(default, skip_serializing_if = "NetfsRedirect::is_default")]
    pub netfs_redirect: NetfsRedirect,
}

impl CacheConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Per-kind path override, if configured.
    pub fn path_for(&self, kind: CacheKind) -> Option<&Path> {
        let p = match kind {
            CacheKind::CondaPackages => &self.conda_packages,
            CacheKind::Repodata => &self.repodata,
            CacheKind::PypiWheels => &self.pypi_wheels,
            CacheKind::PypiMapping => &self.pypi_mapping,
            CacheKind::ExecEnvironments => &self.exec_environments,
            CacheKind::BuildToolEnvironments => &self.build_tool_environments,
            CacheKind::DetachedEnvironments => &self.detached_environments,
        };
        p.as_deref()
    }

    /// Merge `other` on top of `self`, with `other` taking priority.
    pub fn merge(self, other: Self) -> Self {
        Self {
            root: other.root.or(self.root),
            conda_packages: other.conda_packages.or(self.conda_packages),
            repodata: other.repodata.or(self.repodata),
            pypi_wheels: other.pypi_wheels.or(self.pypi_wheels),
            pypi_mapping: other.pypi_mapping.or(self.pypi_mapping),
            exec_environments: other.exec_environments.or(self.exec_environments),
            build_tool_environments: other
                .build_tool_environments
                .or(self.build_tool_environments),
            detached_environments: other.detached_environments.or(self.detached_environments),
            netfs_redirect: if other.netfs_redirect == NetfsRedirect::default() {
                self.netfs_redirect
            } else {
                other.netfs_redirect
            },
        }
    }

    /// Iterate over every set `(field-name, path)` pair. Used by
    /// [`Self::expand_paths`] and [`Self::validate`].
    fn iter_paths_mut(&mut self) -> impl Iterator<Item = (&'static str, &mut PathBuf)> {
        [
            ("cache.root", &mut self.root),
            ("cache.conda-packages", &mut self.conda_packages),
            ("cache.repodata", &mut self.repodata),
            ("cache.pypi-wheels", &mut self.pypi_wheels),
            ("cache.pypi-mapping", &mut self.pypi_mapping),
            ("cache.exec-environments", &mut self.exec_environments),
            (
                "cache.build-tool-environments",
                &mut self.build_tool_environments,
            ),
            (
                "cache.detached-environments",
                &mut self.detached_environments,
            ),
        ]
        .into_iter()
        .filter_map(|(name, opt)| opt.as_mut().map(|p| (name, p)))
    }

    /// Expand a leading `~` to the user's home directory in every set path.
    ///
    /// Mirrors the existing behavior of the top-level `detached-environments`
    /// field. Called automatically by [`Config::from_toml`].
    pub fn expand_paths(&mut self) -> miette::Result<()> {
        for (name, path) in self.iter_paths_mut() {
            if !path.to_string_lossy().starts_with('~') {
                continue;
            }
            let home_dir = dirs::home_dir().ok_or_else(|| {
                miette!(
                    "could not resolve home directory for '~' in `{}` = {}",
                    name,
                    path.display()
                )
            })?;
            // Safe unwrap: we just checked the path starts with '~'.
            *path = home_dir.join(path.strip_prefix("~").unwrap());
        }
        Ok(())
    }

    /// Ensure every set path is absolute. `~` should already be expanded by
    /// [`Self::expand_paths`] before calling this.
    pub fn validate(&self) -> miette::Result<()> {
        // iter_paths_mut takes &mut, so reuse the field list locally rather
        // than cloning. Keep this in sync with `iter_paths_mut`.
        let entries: [(&str, Option<&PathBuf>); 8] = [
            ("cache.root", self.root.as_ref()),
            ("cache.conda-packages", self.conda_packages.as_ref()),
            ("cache.repodata", self.repodata.as_ref()),
            ("cache.pypi-wheels", self.pypi_wheels.as_ref()),
            ("cache.pypi-mapping", self.pypi_mapping.as_ref()),
            ("cache.exec-environments", self.exec_environments.as_ref()),
            (
                "cache.build-tool-environments",
                self.build_tool_environments.as_ref(),
            ),
            (
                "cache.detached-environments",
                self.detached_environments.as_ref(),
            ),
        ];
        for (name, path) in entries.into_iter().filter_map(|(n, p)| p.map(|p| (n, p))) {
            if !path.is_absolute() {
                return Err(miette!(
                    "`{}` must be an absolute path, got: {}",
                    name,
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

/// Policy for cache redirection when the cache root sits on a network
/// filesystem.
#[derive(Default, Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NetfsRedirect {
    /// Redirect kinds that don't [`CacheKind::prefers_shared`] to node-local
    /// scratch. Default.
    #[default]
    Auto,
    /// Always redirect every kind to node-local scratch when on netfs.
    Always,
    /// Never redirect. Equivalent to `PIXI_DISABLE_NETFS_REDIRECT=1`.
    Never,
}

impl NetfsRedirect {
    fn is_default(&self) -> bool {
        *self == Self::Auto
    }
}

impl FromStr for NetfsRedirect {
    type Err = serde::de::value::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::deserialize(s.into_deserializer())
    }
}

/// Per-kind cache path env var name. Setting one is equivalent to
/// `[cache.<kind>]` in `config.toml` and takes precedence over it.
fn env_var_for(kind: CacheKind) -> &'static str {
    match kind {
        CacheKind::CondaPackages => "PIXI_CACHE_CONDA_PACKAGES_DIR",
        CacheKind::Repodata => "PIXI_CACHE_REPODATA_DIR",
        CacheKind::PypiWheels => "PIXI_CACHE_PYPI_WHEELS_DIR",
        CacheKind::PypiMapping => "PIXI_CACHE_PYPI_MAPPING_DIR",
        CacheKind::ExecEnvironments => "PIXI_CACHE_EXEC_ENVIRONMENTS_DIR",
        CacheKind::BuildToolEnvironments => "PIXI_CACHE_BUILD_TOOL_ENVIRONMENTS_DIR",
        CacheKind::DetachedEnvironments => "PIXI_CACHE_DETACHED_ENVIRONMENTS_DIR",
    }
}

/// Read the per-kind path override from the environment, if set.
fn env_path_for(kind: CacheKind) -> Option<PathBuf> {
    std::env::var_os(env_var_for(kind))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Read the netfs-redirect policy from the environment. An unset or
/// unrecognized value yields `None`, deferring to the TOML config.
fn env_netfs_redirect() -> Option<NetfsRedirect> {
    let raw = std::env::var("PIXI_CACHE_NETFS_REDIRECT").ok()?;
    match raw.parse() {
        Ok(mode) => Some(mode),
        Err(_) => {
            tracing::warn!(
                "ignoring PIXI_CACHE_NETFS_REDIRECT={raw}: expected one of \
                 `auto`, `always`, `never`"
            );
            None
        }
    }
}

fn resolve_cache_kind_dir(cache_cfg: &CacheConfig, kind: CacheKind) -> miette::Result<PathBuf> {
    // Env vars override TOML for per-kind paths. Setting one bypasses the
    // redirect logic for that kind, mirroring the TOML field's semantics.
    if let Some(p) = env_path_for(kind) {
        return Ok(p);
    }
    if let Some(p) = cache_cfg.path_for(kind) {
        return Ok(p.to_path_buf());
    }

    let (base, source) = resolve_cache_root(cache_cfg)
        .ok_or_else(|| miette::miette!("could not determine default cache directory"))?;
    let pinned = matches!(source, CacheDirSource::UserPinned);

    let redirect_mode = env_netfs_redirect().unwrap_or(cache_cfg.netfs_redirect);
    let should_redirect = match redirect_mode {
        NetfsRedirect::Never => false,
        NetfsRedirect::Always => is_network_filesystem(&base) && !pinned,
        NetfsRedirect::Auto => !pinned && !kind.prefers_shared() && is_network_filesystem(&base),
    };

    if !should_redirect {
        return Ok(base.join(kind.subdir()));
    }

    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "pixi".to_string());
    let redirected = node_local_scratch_dir()
        .join(format!("pixi-cache-{user}"))
        .join(kind.subdir());

    let original = base.join(kind.subdir());
    let mut warned = NETFS_REDIRECT_WARNED.lock().unwrap();
    if warned.insert(kind) {
        tracing::warn!(
            "cache for {:?} at {} is on a network/parallel filesystem \
             (NFS/SMB/FUSE/BeeGFS/Lustre/GPFS/CephFS), \
             redirected to {} for this run. Set [cache.{}] in config.toml or \
             PIXI_CACHE_DIR to override, or [cache.netfs-redirect] = \"never\" \
             to keep the original path.",
            kind,
            original.display(),
            redirected.display(),
            kind.config_key(),
        );
    }
    Ok(redirected)
}

/// Resolve the cache root, consulting (in order): `PIXI_CACHE_DIR`,
/// `RATTLER_CACHE_DIR`, `[cache.root]` in config, XDG, rattler default.
fn resolve_cache_root(cache_cfg: &CacheConfig) -> Option<(PathBuf, CacheDirSource)> {
    if let Ok(dir) = std::env::var("PIXI_CACHE_DIR") {
        return Some((PathBuf::from(dir), CacheDirSource::UserPinned));
    }
    if let Ok(dir) = std::env::var("RATTLER_CACHE_DIR") {
        return Some((PathBuf::from(dir), CacheDirSource::UserPinned));
    }
    if let Some(root) = &cache_cfg.root {
        return Some((root.clone(), CacheDirSource::UserPinned));
    }
    if let Some(xdg) = dirs::cache_dir()
        .map(|d| d.join(consts::PIXI_DIR))
        .and_then(|d| d.exists().then_some(d))
    {
        return Some((xdg, CacheDirSource::Default));
    }
    rattler::default_cache_dir()
        .ok()
        .map(|p| (p, CacheDirSource::Default))
}

#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigCli {
    /// Path to the file containing the authentication token.
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub auth_file: Option<PathBuf>,

    /// Max concurrent network requests, default is `50`
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub concurrent_downloads: Option<usize>,

    /// Max concurrent solves, default is the number of CPUs
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub concurrent_solves: Option<usize>,

    /// Set pinning strategy
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS, value_enum)]
    pub pinning_strategy: Option<PinningStrategy>,

    /// Specifies whether to use the keyring to look up credentials for PyPI.
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub pypi_keyring_provider: Option<KeyringProvider>,

    /// Run post-link scripts (insecure)
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub run_post_link_scripts: bool,

    /// Disallow symbolic links during package installation
    #[arg(long, env = "PIXI_NO_SYMBOLIC_LINKS", help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub no_symbolic_links: bool,

    /// Disallow hard links during package installation
    #[arg(long, env = "PIXI_NO_HARD_LINKS", help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub no_hard_links: bool,

    /// Disallow ref links (copy-on-write) during package installation
    #[arg(long, env = "PIXI_NO_REF_LINKS", help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub no_ref_links: bool,

    /// Do not verify the TLS certificate of the server.
    #[arg(long, action = ArgAction::SetTrue, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub tls_no_verify: bool,

    /// Which TLS root certificates to use: 'webpki' (bundled Mozilla roots) or 'system' (system store).
    #[arg(long, env = "PIXI_TLS_ROOT_CERTS", value_parser = parse_tls_root_certs_cli, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub tls_root_certs: Option<TlsRootCerts>,

    /// Use environment activation cache (experimental)
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    pub use_environment_activation_cache: bool,
}

#[derive(Parser, Debug, Clone, Default)]
pub struct ConfigCliPrompt {
    /// Do not change the PS1 variable when starting a prompt.
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    change_ps1: Option<bool>,
}

impl From<ConfigCliPrompt> for Config {
    fn from(cli: ConfigCliPrompt) -> Self {
        let mut config = Config::default();
        config.extensions.shell.change_ps1 = cli.change_ps1;
        config
    }
}

impl ConfigCliPrompt {
    pub fn merge_config(self, config: Config) -> Config {
        let mut config = config;
        config.extensions.shell.change_ps1 = self.change_ps1.or(config.extensions.shell.change_ps1);
        config
    }
}

// `RepodataConfig` and `RepodataChannelConfig` now live in
// `rattler_config`. The upstream `RepodataConfig` already includes the
// tolerant `Deserialize` impl that pixi used to maintain locally —
// unknown/deprecated keys (e.g. `disable-jlap`) are silently consumed
// and surface as `serde_ignored` warnings.
pub use rattler_config::config::repodata_config::{RepodataChannelConfig, RepodataConfig};

#[derive(Parser, Debug, Default, Clone)]
pub struct ConfigCliActivation {
    /// Do not use the environment activation cache. (default: true except in
    /// experimental mode)
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    force_activate: bool,

    /// Do not source the autocompletion scripts from the environment.
    #[arg(long, help_heading = consts::CLAP_CONFIG_OPTIONS)]
    no_completions: bool,
}

impl ConfigCliActivation {
    pub fn merge_config(self, config: Config) -> Config {
        let mut config = config;
        config.extensions.shell.force_activate = Some(self.force_activate);
        if self.no_completions {
            config.extensions.shell.source_completion_scripts = Some(false);
        }
        config
    }
}

// Convert a `RepodataChannelConfig` into rattler's `SourceConfig`.
// Used to be a `From` impl, but both types are now foreign — orphan
// rule means we need a free function. Call sites use it explicitly.
fn repodata_channel_to_source(value: RepodataChannelConfig) -> SourceConfig {
    SourceConfig {
        zstd_enabled: !value.disable_zstd.unwrap_or(false),
        bz2_enabled: !value.disable_bzip2.unwrap_or(false),
        sharded_enabled: !value.disable_sharded.unwrap_or(false),
        cache_action: Default::default(),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum KeyringProvider {
    Disabled,
    Subprocess,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct PyPIConfig {
    /// The default index URL for PyPI packages.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_url: Option<Url>,
    /// A list of extra index URLs for PyPI packages
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_index_urls: Vec<Url>,
    /// Whether to use the `keyring` executable to look up credentials.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyring_provider: Option<KeyringProvider>,
    /// Allow insecure connections to a host
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_insecure_host: Vec<String>,
}

// `S3Options` and the `S3OptionsMap` newtype now live in `rattler_config`.
// Re-exported so external crates that referenced `pixi_config::S3Options`
// keep compiling.
pub use rattler_config::config::s3::{S3Options, S3OptionsMap};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum DetachedEnvironments {
    Boolean(bool),
    Path(PathBuf),
}

impl DetachedEnvironments {
    pub fn is_false(&self) -> bool {
        matches!(self, DetachedEnvironments::Boolean(false))
    }

    // Get the path to the detached-environments directory. None means the default
    // directory.
    //
    // `cache` is the resolved cache config to consult when the default
    // directory has to be derived (i.e. when detached-environments is just
    // enabled via a boolean), so workspace-level `[cache.detached-environments]`
    // overrides are honored. Callers should use
    // [`Config::detached_environments_dir`] rather than calling this directly.
    fn path(&self, cache: &CacheConfig) -> miette::Result<Option<PathBuf>> {
        let resolved_self = self.resolve_path()?;
        match resolved_self {
            DetachedEnvironments::Path(p) => Ok(Some(p.clone())),
            DetachedEnvironments::Boolean(b) if b => Ok(Some(resolve_cache_kind_dir(
                cache,
                CacheKind::DetachedEnvironments,
            )?)),
            _ => Ok(None),
        }
    }

    /// If `self` is the `DetachedEnvironments::Path` variant, expands `~`
    /// to the absolute path to the home directory, otherwise clone the boolean
    /// variant.
    pub fn resolve_path(&self) -> miette::Result<Self> {
        match self {
            DetachedEnvironments::Boolean(_) => Ok(self.clone()),
            DetachedEnvironments::Path(p) => {
                let mut path = p.clone();
                // If the path starts with ~, expand it to the home directory
                if path.to_string_lossy().starts_with("~") {
                    let home_dir = dirs::home_dir().ok_or_else(|| {
                        miette!(
                            "Could not resolve home directory for '~' in path {}",
                            path.display()
                        )
                    })?;
                    // Safe unwrap as we checked if it starts with ~
                    path = home_dir.join(path.strip_prefix("~").unwrap());
                }
                Ok(DetachedEnvironments::Path(path))
            }
        }
    }

    pub fn validate(&self) -> miette::Result<()> {
        // Resolve the path variant (if present) prior to validating it.
        let resolved_self = self.resolve_path()?;

        match resolved_self {
            DetachedEnvironments::Boolean(_) => {}
            DetachedEnvironments::Path(path) => {
                if !path.is_absolute() {
                    return Err(miette!(
                        "The `detached-environments` path must be an absolute path: {}",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Default for DetachedEnvironments {
    fn default() -> Self {
        DetachedEnvironments::Boolean(false)
    }
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ExperimentalConfig {
    /// The option to opt into the environment activation cache feature.
    /// This is an experimental feature and may be removed in the future or made
    /// default.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_environment_activation_cache: Option<bool>,
}

impl ExperimentalConfig {
    pub fn merge(self, other: Self) -> Self {
        Self {
            use_environment_activation_cache: other
                .use_environment_activation_cache
                .or(self.use_environment_activation_cache),
        }
    }
    pub fn use_environment_activation_cache(&self) -> bool {
        self.use_environment_activation_cache.unwrap_or(false)
    }

    pub fn is_default(&self) -> bool {
        self.use_environment_activation_cache.is_none()
    }
}

// `ConcurrencyConfig` and its default helpers now live in `rattler_config`.
// Re-exported so external code keeps compiling against `pixi_config::…`.
pub use rattler_config::config::concurrency::{
    ConcurrencyConfig, default_max_concurrent_downloads, default_max_concurrent_solves,
};

impl PyPIConfig {
    /// Merge the given PyPIConfig into the current one.
    pub fn merge(self, other: Self) -> Self {
        let extra_index_urls = self
            .extra_index_urls
            .into_iter()
            .chain(other.extra_index_urls)
            .collect();

        Self {
            index_url: other.index_url.or(self.index_url),
            extra_index_urls,
            keyring_provider: other.keyring_provider.or(self.keyring_provider),
            allow_insecure_host: self
                .allow_insecure_host
                .into_iter()
                .chain(other.allow_insecure_host)
                .collect(),
        }
    }

    pub fn with_keyring(mut self, keyring_provider: KeyringProvider) -> Self {
        self.keyring_provider = Some(keyring_provider);
        self
    }

    /// Whether to use the `keyring` executable to look up credentials.
    /// Defaults to false.
    pub fn use_keyring(&self) -> KeyringProvider {
        self.keyring_provider
            .clone()
            .unwrap_or(KeyringProvider::Disabled)
    }

    fn is_default(&self) -> bool {
        self.index_url.is_none()
            && self.extra_index_urls.is_empty()
            && self.keyring_provider.is_none()
            && self.allow_insecure_host.is_empty()
    }
}

/// The strategy for that will be used for pinning a version of a package.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, Copy, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum PinningStrategy {
    /// Default semver strategy e.g. `1.2.3` becomes `>=1.2.3, <2` but `0.1.0`
    /// becomes `>=0.1.0, <0.2`
    #[default]
    Semver,
    /// Pin the latest minor e.g. `1.2.3` becomes `>=1.2.3, <1.3`
    Minor,
    /// Pin the latest major e.g. `1.2.3` becomes `>=1.2.3, <2`
    Major,
    /// Pin to the latest version or higher. e.g. `1.2.3` becomes `>=1.2.3`
    LatestUp,
    /// Pin the version chosen by the solver. e.g. `1.2.3` becomes `==1.2.3`
    // Adding "Version" to the name for future extendability.
    ExactVersion,
    /// No pinning, keep the requirement empty. e.g. `1.2.3` becomes `*`
    // Calling it no-pin to make it simple to type, as other option was pin-unconstrained.
    NoPin,
}
impl FromStr for PinningStrategy {
    type Err = serde::de::value::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::deserialize(s.into_deserializer())
    }
}

impl PinningStrategy {
    /// Given a set of versions, determines the best version constraint to use
    /// that captures all of them based on the strategy.
    pub fn determine_version_constraint<'a>(
        self,
        versions: impl IntoIterator<Item = &'a Version> + Clone,
    ) -> Option<VersionSpec> {
        let (min_version, max_version) = versions.clone().into_iter().minmax().into_option()?;
        let lower_bound = min_version.clone();
        let num_segments = max_version.segment_count();

        let constraint = match self {
            Self::ExactVersion => VersionSpec::Group(
                LogicalOperator::Or,
                versions
                    .into_iter()
                    .dedup()
                    .map(|v| VersionSpec::Exact(EqualityOperator::Equals, v.clone()))
                    .collect(),
            ),
            Self::Major => {
                let upper_bound = max_version
                    .clone()
                    .pop_segments(num_segments.saturating_sub(1))
                    .unwrap_or(max_version.to_owned())
                    .bump(VersionBumpType::Major)
                    .ok()?;
                VersionSpec::Group(
                    LogicalOperator::And,
                    vec![
                        VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
                        VersionSpec::Range(RangeOperator::Less, upper_bound),
                    ],
                )
            }
            Self::Minor => {
                let upper_bound = max_version
                    .clone()
                    .pop_segments(num_segments.saturating_sub(2))
                    .unwrap_or(max_version.to_owned())
                    .bump(VersionBumpType::Minor)
                    .ok()?;
                VersionSpec::Group(
                    LogicalOperator::And,
                    vec![
                        VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
                        VersionSpec::Range(RangeOperator::Less, upper_bound),
                    ],
                )
            }
            Self::LatestUp => VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
            Self::NoPin => VersionSpec::Any,
            Self::Semver => {
                // Pin the first left most non-zero segment for the upperbound
                let mut left_most_non_zero_offset = None;
                for (index, segment) in max_version.segments().enumerate() {
                    if !segment.is_zero() {
                        left_most_non_zero_offset = Some(index);
                        break;
                    }
                }
                let upper_bound = max_version
                    .with_segments(
                        0..=left_most_non_zero_offset.unwrap_or(max_version.segment_count()),
                    )
                    .unwrap_or(max_version.clone())
                    .bump(VersionBumpType::Last)
                    .ok()?;
                VersionSpec::Group(
                    LogicalOperator::And,
                    vec![
                        VersionSpec::Range(RangeOperator::GreaterEquals, lower_bound),
                        VersionSpec::Range(RangeOperator::Less, upper_bound),
                    ],
                )
            }
        };
        Some(constraint)
    }
}

// `RunPostLinkScripts` now lives in `rattler_config`. Re-exported so
// `pixi_config::RunPostLinkScripts` remains a valid path for external
// crates (pixi_core, pixi_global).
pub use rattler_config::config::run_post_link_scripts::RunPostLinkScripts;

/// Pixi-specific configuration keys, layered on top of the shared rattler
/// config as the *extension* type of [`ConfigBase`].
///
/// These keys live at the top level of the same `config.toml` document as the
/// shared keys of [`CommonConfig`]. The serde attributes (kebab-case names,
/// snake_case aliases, `skip_serializing_if` guards) are carried over
/// unchanged from the former monolithic `Config` struct so the on-disk format
/// stays identical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct PixiConfig {
    /// Dependency Pinning strategy used for dependency modification through
    /// automated logic like `pixi add`
    // TODO(rattler-config): promote — useful for any tool that
    // adds/updates conda deps. rattler_config does not model this yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinning_strategy: Option<PinningStrategy>,

    /// Configuration for PyPI packages.
    #[serde(default)]
    #[serde(skip_serializing_if = "PyPIConfig::is_default")]
    pub pypi_config: PyPIConfig,

    /// The option to specify the directory where detached environments are
    /// stored. When using 'true', it defaults to the cache directory.
    /// When using a path, it uses the specified path.
    /// When using 'false', it disables detached environments, meaning it moves
    /// it back to the .pixi folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached_environments: Option<DetachedEnvironments>,

    /// Shell-specific configuration
    #[serde(default)]
    #[serde(skip_serializing_if = "ShellConfig::is_default")]
    pub shell: ShellConfig,

    /// Experimental features that can be enabled.
    #[serde(default)]
    #[serde(skip_serializing_if = "ExperimentalConfig::is_default")]
    pub experimental: ExperimentalConfig,

    /// The platform to use when installing tools.
    ///
    /// When running on certain platforms, you might want to install build
    /// backends and other tools for a different platform than the current one.
    /// Using this field, you can specify the platform that is used to install
    /// these types of tools.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_platform: Option<Platform>,

    /// Per-cache directory configuration. Lets users redirect specific
    /// caches (conda packages, repodata, pypi mapping, etc.) to different
    /// locations — useful on HPC where the package cache should live on a
    /// shared filesystem but transient caches should stay node-local.
    #[serde(default)]
    #[serde(skip_serializing_if = "CacheConfig::is_default")]
    pub cache: CacheConfig,

    //////////////////////
    // Deprecated fields //
    //////////////////////
    /// Deprecated; folded into `shell.change-ps1` at load time.
    #[serde(default)]
    #[serde(alias = "change_ps1")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_ps1: Option<bool>,

    /// Deprecated; folded into `shell.force-activate` at load time.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_activate: Option<bool>,
}

impl RattlerConfig for PixiConfig {
    /// Merge another configuration on top of this one. `other` takes
    /// priority, matching the semantics of the former `Config::merge_config`.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            pinning_strategy: other.pinning_strategy.or(self.pinning_strategy),
            pypi_config: self.pypi_config.merge(other.pypi_config.clone()),
            detached_environments: other
                .detached_environments
                .clone()
                .or(self.detached_environments),
            shell: self.shell.merge(other.shell.clone()),
            experimental: self.experimental.merge(other.experimental.clone()),
            // NOTE: `self` wins here — this preserves the (long-standing)
            // behavior of the former `Config::merge_config`.
            tool_platform: self.tool_platform.or(other.tool_platform),
            cache: self.cache.clone().merge(other.cache.clone()),

            // Deprecated fields are dropped on merge: `Config::from_toml`
            // folds them into `shell.*` before configs are merged.
            change_ps1: None,
            force_activate: None,
        })
    }

    /// The dotted TOML key paths of the pixi-specific keys, used by the
    /// generic [`ConfigBase::set`] for its "supported keys" listing and
    /// unknown-key detection.
    fn keys(&self) -> Vec<String> {
        [
            "pinning-strategy",
            "detached-environments",
            "tool-platform",
            "pypi-config.index-url",
            "pypi-config.extra-index-urls",
            "pypi-config.keyring-provider",
            "pypi-config.allow-insecure-host",
            "shell.change-ps1",
            "shell.force-activate",
            "shell.source-completion-scripts",
            "experimental.use-environment-activation-cache",
            "cache.root",
            "cache.conda-packages",
            "cache.repodata",
            "cache.pypi-wheels",
            "cache.pypi-mapping",
            "cache.exec-environments",
            "cache.build-tool-environments",
            "cache.detached-environments",
            "cache.netfs-redirect",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
}

/// The pixi configuration: the shared rattler configuration
/// ([`CommonConfig`]) extended with the pixi-specific keys ([`PixiConfig`]).
///
/// This is a thin wrapper around [`ConfigBase<PixiConfig>`] that carries
/// pixi's inherent methods (loading, merging semantics, cache resolution,
/// clap integration, ...). It dereferences to [`ConfigBase`], which itself
/// dereferences to [`CommonConfig`], so both the shared fields
/// (`config.mirrors`, `config.concurrency`, ...) and the extension
/// (`config.extensions.shell`, ...) remain directly accessible.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Config {
    inner: ConfigBase<PixiConfig>,
}

impl Deref for Config {
    type Target = ConfigBase<PixiConfig>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl From<ConfigBase<PixiConfig>> for Config {
    fn from(inner: ConfigBase<PixiConfig>) -> Self {
        Self { inner }
    }
}

/// Emit a deprecation warning when a config layer (file, CLI flag, or env
/// var) sets `tls-root-certs` to one of the deprecated spellings.
///
/// The shared [`TlsRootCerts`] enum accepts `"native"` and `"all"` as plain
/// aliases of `System`, so the legacy spellings can no longer be detected
/// from the parsed value; callers pass the raw string instead
/// ([`Config::from_toml`] inspects the raw TOML document).
///
/// `source` is included in the message so users can find where the bad value
/// came from. Pass `None` for non-file sources (CLI / env).
fn warn_deprecated_tls_root_certs(raw: &str, source: Option<&str>) {
    let (old, advice): (&str, String) = match raw {
        "native" => (
            "tls-root-certs = \"native\"",
            format!(
                "rename to '{}'",
                console::style("tls-root-certs = \"system\"").green()
            ),
        ),
        "all" => (
            "tls-root-certs = \"all\"",
            format!(
                "merging webpki and system roots is no longer supported. \
                 Pick one of '{}' or '{}', or set {} / {}. \
                 The value falls back to 'system' for now.",
                console::style("webpki").green(),
                console::style("system").green(),
                console::style("SSL_CERT_FILE").green(),
                console::style("SSL_CERT_DIR").green(),
            ),
        ),
        _ => return,
    };
    let msg = format!("'{}' is deprecated: {advice}", console::style(old).red(),);
    match source {
        Some(src) => tracing::warn!("In '{}': {msg}", console::style(src).bold()),
        None => tracing::warn!("{msg}"),
    }
}

/// Emit the `tls-root-certs` deprecation warnings for a raw TOML document.
///
/// The shared [`TlsRootCerts`] deserializer silently maps the legacy
/// `"native"` / `"all"` spellings to `System`, so the raw document is
/// inspected to keep the load-time warnings.
fn warn_deprecated_tls_root_certs_in_document(toml: &str, source_path: Option<&Path>) {
    let Ok(document) = toml.parse::<toml_edit::DocumentMut>() else {
        return;
    };
    for key in ["tls-root-certs", "tls_root_certs"] {
        if let Some(raw) = document.get(key).and_then(|v| v.as_str()) {
            warn_deprecated_tls_root_certs(
                raw,
                source_path.map(|p| p.display().to_string()).as_deref(),
            );
        }
    }
}

impl From<ConfigCli> for Config {
    fn from(cli: ConfigCli) -> Self {
        // Note: the deprecation warning for legacy `--tls-root-certs`
        // spellings fires in the clap value parser
        // (`parse_tls_root_certs_cli`), where the raw string is available.
        Config {
            inner: ConfigBase {
                common: CommonConfig {
                    tls_no_verify: if cli.tls_no_verify { Some(true) } else { None },
                    tls_root_certs: cli.tls_root_certs,
                    authentication_override_file: cli.auth_file,
                    concurrency: ConcurrencyConfig {
                        solves: cli
                            .concurrent_solves
                            .unwrap_or(ConcurrencyConfig::default().solves),
                        downloads: cli
                            .concurrent_downloads
                            .unwrap_or(ConcurrencyConfig::default().downloads),
                    },
                    run_post_link_scripts: if cli.run_post_link_scripts {
                        Some(RunPostLinkScripts::Insecure)
                    } else {
                        None
                    },
                    allow_symbolic_links: cli.no_symbolic_links.then_some(false),
                    allow_hard_links: cli.no_hard_links.then_some(false),
                    allow_ref_links: cli.no_ref_links.then_some(false),
                    ..CommonConfig::default()
                },
                extensions: PixiConfig {
                    pypi_config: cli
                        .pypi_keyring_provider
                        .map(|val| PyPIConfig::default().with_keyring(val))
                        .unwrap_or_default(),
                    experimental: ExperimentalConfig {
                        use_environment_activation_cache: if cli.use_environment_activation_cache {
                            Some(true)
                        } else {
                            None
                        },
                    },
                    pinning_strategy: cli.pinning_strategy,
                    ..PixiConfig::default()
                },
                loaded_from: Vec::new(),
            },
        }
    }
}

impl From<Config> for rattler_repodata_gateway::ChannelConfig {
    fn from(config: Config) -> Self {
        rattler_repodata_gateway::ChannelConfig::from(&config)
    }
}

impl From<&Config> for rattler_repodata_gateway::ChannelConfig {
    fn from(config: &Config) -> Self {
        let repodata_config = &config.repodata_config;
        let default = repodata_channel_to_source(repodata_config.default.clone());

        let per_channel = repodata_config
            .per_channel
            .iter()
            .map(|(url, config)| {
                (
                    url.clone(),
                    repodata_channel_to_source(config.merge(repodata_config.default.clone())),
                )
            })
            .collect();

        rattler_repodata_gateway::ChannelConfig {
            default,
            per_channel,
        }
    }
}

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ShellConfig {
    /// The option to disable the environment activation cache
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_activate: Option<bool>,

    /// Whether to source completion scripts from the environment or not.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    source_completion_scripts: Option<bool>,

    /// If set to true, pixi will set the PS1 environment variable to a custom
    /// value.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_ps1: Option<bool>,
}

impl ShellConfig {
    pub fn merge(self, other: Self) -> Self {
        Self {
            force_activate: other.force_activate.or(self.force_activate),
            source_completion_scripts: other
                .source_completion_scripts
                .or(self.source_completion_scripts),
            change_ps1: other.change_ps1.or(self.change_ps1),
        }
    }

    pub fn is_default(&self) -> bool {
        self.force_activate.is_none()
            && self.source_completion_scripts.is_none()
            && self.change_ps1.is_none()
    }

    pub fn source_completion_scripts(&self) -> bool {
        self.source_completion_scripts.unwrap_or(true)
    }
}

// `ProxyConfig` now lives in `rattler_config`. Re-exported for
// back-compat. Note: rattler's `Default::default()` reads `HTTP_PROXY`
// / `HTTPS_PROXY` / `NO_PROXY` env vars into the struct, whereas
// pixi's old `Default` was empty. The local `ENV_*_PROXY` / `USE_PROXY_FROM_ENV`
// statics below are kept because `get_proxies()` and the load-time
// warning still consult env vars directly to decide whether to defer
// to reqwest's own env-var handling. We end up reading the env twice
// per process (once cached in rattler, once cached here) — acceptable.
pub use rattler_config::config::proxy::ProxyConfig;

// `BuildConfig` and `PackageFormatAndCompression` now live in
// `rattler_config`. Re-exported so external paths keep compiling.
pub use rattler_config::config::build::{BuildConfig, PackageFormatAndCompression};

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("no file was found at {0}")]
    FileNotFound(PathBuf),
    #[error("failed to read config from '{0}'")]
    ReadError(std::io::Error),
    #[error("failed to parse config of {1}: {0}")]
    ParseError(miette::Report, PathBuf),
    #[error("validation error of {1}: {0}")]
    ValidationError(miette::Report, PathBuf),
}

impl Config {
    /// Constructs a new config that is optimized to be used in tests.
    ///
    /// This instance is optimized to provide the fastest experience for tests.
    pub fn for_tests() -> Self {
        let mut config = Config::default();
        // Use prefix.dev as the default channel alias
        config.channel_config.channel_alias = Url::parse("https://prefix.dev").unwrap();

        // Use conda-forge as the default channel
        config.common.default_channels = Some(vec![NamedChannelOrUrl::Name("conda-forge".into())]);

        // Enable sharded repodata by default.
        config.repodata_config.default.disable_sharded = Some(false);

        // HACK: Use win-64 as the default tool platform if currently running on
        // win-arm64. This is a workaround for the fact that we don't have a
        // good win-arm64 toolchain yet.
        if Platform::current() == Platform::WinArm64 {
            config.extensions.tool_platform = Some(Platform::Win64);
        }

        config
    }

    /// Parse the given toml string and return a Config instance.
    ///
    /// # Returns
    ///
    /// The parsed config, and the unused keys
    ///
    /// # Errors
    ///
    /// Parsing errors
    #[inline]
    pub fn from_toml(
        toml: &str,
        source_path: Option<&Path>,
    ) -> miette::Result<(Config, Set<String>)> {
        // Parse through the shared two-pass deserializer: keys are consumed
        // by either the common configuration or the pixi extension; keys
        // that neither recognizes (typos, keys of other tools) are returned
        // so callers can warn about them.
        let (inner, unused_keys) =
            ConfigBase::<PixiConfig>::from_toml_str(toml).into_diagnostic()?;
        let mut config = Config { inner };

        fn create_deprecation_warning(old: &str, new: &str, source_path: Option<&Path>) {
            let msg = format!(
                "Please replace '{}' with '{}', the field is deprecated and will be removed in a future release.",
                console::style(old).red(),
                console::style(new).green()
            );
            match source_path {
                Some(path) => {
                    tracing::warn!("In '{}': {}", console::style(path.display()).bold(), msg,)
                }
                None => tracing::warn!("{}", msg),
            }
        }

        if config.extensions.change_ps1.is_some() {
            create_deprecation_warning("change-ps1", "shell.change-ps1", source_path);
            config.extensions.shell.change_ps1 = config.extensions.change_ps1;
        }

        if config.extensions.force_activate.is_some() {
            create_deprecation_warning("force-activate", "shell.force-activate", source_path);
            config.extensions.shell.force_activate = config.extensions.force_activate;
        }

        // The shared `TlsRootCerts` deserializer accepts the legacy
        // `"native"` / `"all"` spellings as silent aliases of `system`;
        // inspect the raw document to keep pixi's deprecation warnings.
        warn_deprecated_tls_root_certs_in_document(toml, source_path);

        // An explicitly empty channel list is treated the same as an absent
        // one, preserving the merge semantics of the former `Config` (where
        // the field was a plain `Vec` and empty meant "not set").
        if config
            .common
            .default_channels
            .as_ref()
            .is_some_and(Vec::is_empty)
        {
            config.common.default_channels = None;
        }

        // Expand `~` in every [cache] path, matching how the top-level
        // `detached-environments` field is handled. Validation that the
        // expanded paths are absolute happens in `Config::validate`.
        config.extensions.cache.expand_paths()?;

        Ok((config, unused_keys))
    }

    /// Load the config from the given path.
    ///
    /// # Returns
    ///
    /// The loaded config
    ///
    /// # Errors
    ///
    /// I/O errors or parsing errors
    pub fn from_path(path: &Path) -> Result<Config, ConfigError> {
        tracing::debug!("Loading config from {}", path.display());
        let s = match fs_err::read_to_string(path) {
            Ok(content) => content,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::NotADirectory =>
            {
                return Err(ConfigError::FileNotFound(path.to_path_buf()));
            }
            Err(e) => return Err(ConfigError::ReadError(e)),
        };

        let (mut config, unused_keys) = Config::from_toml(&s, Some(path))
            .map_err(|e| ConfigError::ParseError(e, path.to_path_buf()))?;

        if !unused_keys.is_empty() {
            tracing::warn!(
                "Ignoring '{}' in {}",
                console::style(
                    unused_keys
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .yellow(),
                path.display()
            );
        }

        config.loaded_from.push(path.to_path_buf());
        tracing::debug!("Loaded config from: {}", path.display());

        config
            .validate()
            .map_err(|e| ConfigError::ValidationError(e, path.to_path_buf()))?;

        // check proxy config
        if config.proxy_config.https.is_none() && config.proxy_config.http.is_none() {
            if !config.proxy_config.non_proxy_hosts.is_empty() {
                tracing::warn!(
                    "proxy-config.non-proxy-hosts is not empty but will be ignored, as no https or http config is set."
                )
            }
        } else if *USE_PROXY_FROM_ENV {
            let config_no_proxy = Some(config.proxy_config.non_proxy_hosts.iter().join(","))
                .filter(|v| !v.is_empty());
            if (*ENV_HTTPS_PROXY).as_deref() != config.proxy_config.https.as_ref().map(Url::as_str)
                || (*ENV_HTTP_PROXY).as_deref()
                    != config.proxy_config.http.as_ref().map(Url::as_str)
                || *ENV_NO_PROXY != config_no_proxy
            {
                tracing::info!("proxy configs are overridden by proxy environment vars.")
            }
        }

        Ok(config)
    }

    /// Try to load the system config file from the system path.
    ///
    /// # Returns
    ///
    /// The loaded system config
    ///
    /// # Errors
    ///
    /// I/O errors or parsing errors
    pub fn try_load_system() -> Result<Config, ConfigError> {
        Self::from_path(&config_path_system())
    }

    /// Load the system config file from the system path.
    ///
    /// # Returns
    ///
    /// The loaded system config
    pub fn load_system() -> Config {
        Self::try_load_system().unwrap_or_else(|e| {
            match e {
                ConfigError::FileNotFound(_) => (), // it's fine that no file is there
                e => tracing::error!("{e}"),
            }

            Self::default()
        })
    }

    /// Validate the config file.
    pub fn validate(&self) -> miette::Result<()> {
        // Validate the detached environments directory is set correctly
        if let Some(detached_environments) = self.extensions.detached_environments.as_ref() {
            detached_environments.validate()?
        }

        // Validate that all configured [cache] paths are absolute.
        self.extensions.cache.validate()?;

        Ok(())
    }

    /// Load the global (system + user-level) config layer using the given
    /// source.
    ///
    /// - [`GlobalConfigSource::None`]: return [`Config::default`].
    /// - [`GlobalConfigSource::File`]: load only that file.
    /// - [`GlobalConfigSource::Search`]: load `/etc/pixi/config.toml` and
    ///   every entry in [`config_path_global`], merging them in order. This
    ///   case is cached process-wide because the underlying files are
    ///   env-independent.
    ///
    /// The project-local `<project>/.pixi/config.toml` layer that
    /// [`Config::load_with`] adds on top is unaffected.
    pub fn load_global_with(source: &GlobalConfigSource) -> Config {
        // Cache only the default search-the-disk layers; non-default sources
        // (--no-config / --config-file) are per-invocation and can vary.
        static SEARCH_LAYERS: LazyLock<Config> = LazyLock::new(|| {
            let mut config = Config::load_system();
            for p in config_path_global() {
                match Config::from_path(&p) {
                    Ok(c) => config = config.merge_config(c),
                    Err(ConfigError::FileNotFound(_)) => (),
                    Err(e) => tracing::error!(
                        "Failed to load global config '{}' with error: {}",
                        p.display(),
                        e
                    ),
                }
            }
            config
        });

        let file_layers = match source {
            GlobalConfigSource::None => Config::default(),
            GlobalConfigSource::File(path) => match Config::from_path(path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        "Failed to load config file '{}' (from --config-file / \
                         PIXI_CONFIG_FILE): {}",
                        path.display(),
                        e
                    );
                    Config::default()
                }
            },
            GlobalConfigSource::Search => SEARCH_LAYERS.clone(),
        };

        // Layer env-derived CLI defaults so process-env changes still apply.
        let mut default_cli = ConfigCli::default();
        default_cli.update_from(std::env::args().take(0));
        file_layers.merge_config(default_cli.into())
    }

    /// Load the global config layer using the default search behavior.
    pub fn load_global() -> Config {
        Self::load_global_with(&GlobalConfigSource::Search)
    }

    /// Load the global config and layer the given cli config on top of it.
    pub fn with_cli_config(cli: &ConfigCli) -> Config {
        let config = Config::load_global();
        config.merge_config(cli.clone().into())
    }

    /// Load the config from the given path (project root), using the supplied
    /// source for the global layer. Project-local
    /// `<project>/.pixi/config.toml` is merged on top.
    pub fn load_with(project_root: &Path, source: &GlobalConfigSource) -> Config {
        let mut config = Self::load_global_with(source);
        let local_config_path = project_root
            .join(consts::PIXI_DIR)
            .join(consts::CONFIG_FILE);

        match Self::from_path(&local_config_path) {
            Ok(c) => config = config.merge_config(c),
            Err(e) => tracing::debug!(
                "Failed to load local config: {} (error: {})",
                local_config_path.display(),
                e
            ),
        }

        config
    }

    /// Load the config from the given path (project root) using the default
    /// global search.
    pub fn load(project_root: &Path) -> Config {
        Self::load_with(project_root, &GlobalConfigSource::Search)
    }

    // Get all possible keys of the configuration
    pub fn get_keys(&self) -> &[&str] {
        &[
            "authentication-override-file",
            "cache",
            "cache.build-tool-environments",
            "cache.conda-packages",
            "cache.detached-environments",
            "cache.exec-environments",
            "cache.netfs-redirect",
            "cache.pypi-mapping",
            "cache.pypi-wheels",
            "cache.repodata",
            "cache.root",
            "concurrency",
            "concurrency.downloads",
            "concurrency.solves",
            "default-channels",
            "detached-environments",
            "experimental",
            "experimental.use-environment-activation-cache",
            "mirrors",
            "pinning-strategy",
            "proxy-config",
            "proxy-config.http",
            "proxy-config.https",
            "proxy-config.non-proxy-hosts",
            "pypi-config",
            "pypi-config.allow-insecure-host",
            "pypi-config.extra-index-urls",
            "pypi-config.index-url",
            "pypi-config.keyring-provider",
            "repodata-config",
            "repodata-config.disable-bzip2",
            "repodata-config.disable-sharded",
            "repodata-config.disable-zstd",
            "run-post-link-scripts",
            "allow-symbolic-links",
            "allow-hard-links",
            "allow-ref-links",
            "s3-options",
            "s3-options.<bucket>",
            "s3-options.<bucket>.endpoint-url",
            "s3-options.<bucket>.force-path-style",
            "s3-options.<bucket>.region",
            "shell",
            "shell.change-ps1",
            "shell.force-activate",
            "shell.source-completion-scripts",
            "tls-no-verify",
            "tls-root-certs",
            "tool-platform",
        ]
    }

    /// Merge the `other` config into `self`.
    /// The `other` config will have higher priority
    #[must_use]
    pub fn merge_config(self, other: Config) -> Self {
        // The shared merge (`ConfigBase::merge_config`) handles both the
        // common fields and the pixi extension (via `PixiConfig`'s
        // `RattlerConfig` impl). The only pixi-specific deviation is the
        // handling of the serde-skipped `channel_config`, which the shared
        // merge always takes from `self`.
        let self_channel_config = self.inner.common.channel_config.clone();
        let other_channel_config = other.inner.common.channel_config.clone();

        let mut merged = self
            .inner
            .merge_config(&other.inner)
            .expect("merging pixi configs is infallible");

        merged.common.channel_config = if other_channel_config == default_channel_config() {
            self_channel_config
        } else {
            other_channel_config
        };

        Config { inner: merged }
    }

    /// Retrieve the value for the default_channels field (defaults to the
    /// ["conda-forge"]).
    pub fn default_channels(&self) -> Vec<NamedChannelOrUrl> {
        match &self.common.default_channels {
            Some(channels) if !channels.is_empty() => channels.clone(),
            _ => consts::DEFAULT_CHANNELS.clone(),
        }
    }

    /// Retrieve the value for the tls_no_verify field (defaults to false).
    pub fn tls_no_verify(&self) -> bool {
        self.tls_no_verify.unwrap_or(false)
    }

    /// The user-set `tls-root-certs` value, if any.
    ///
    /// Returns `None` when the field was not set in any config layer. The
    /// backend-aware fallback lives in `pixi_utils::reqwest::default_tls_root_certs`,
    /// since the choice depends on whether pixi is built against `native-tls`
    /// or `rustls` and this crate is feature-agnostic.
    pub fn tls_root_certs(&self) -> Option<TlsRootCerts> {
        self.common.tls_root_certs
    }

    /// Retrieve the value for the change_ps1 field (defaults to true).
    pub fn change_ps1(&self) -> bool {
        self.extensions.shell.change_ps1.unwrap_or(true)
    }

    /// Retrieve the value for the auth_file field.
    pub fn authentication_override_file(&self) -> Option<&PathBuf> {
        self.authentication_override_file.as_ref()
    }

    /// Returns the global channel configuration.
    ///
    /// This roots the channel configuration to the current directory. When
    /// working with a project though the channel configuration should be rooted
    /// in the project directory.
    pub fn global_channel_config(&self) -> &ChannelConfig {
        &self.channel_config
    }

    pub fn repodata_config(&self) -> &RepodataConfig {
        &self.repodata_config
    }

    pub fn pypi_config(&self) -> &PyPIConfig {
        &self.extensions.pypi_config
    }

    /// Shell-specific configuration.
    pub fn shell(&self) -> &ShellConfig {
        &self.extensions.shell
    }

    /// Experimental features that can be enabled.
    pub fn experimental(&self) -> &ExperimentalConfig {
        &self.extensions.experimental
    }

    /// Per-cache directory configuration.
    pub fn cache(&self) -> &CacheConfig {
        &self.extensions.cache
    }

    /// Dependency pinning strategy used by e.g. `pixi add`, if configured.
    pub fn pinning_strategy(&self) -> Option<PinningStrategy> {
        self.extensions.pinning_strategy
    }

    pub fn mirror_map(&self) -> &IndexMap<Url, Vec<Url>> {
        &self.common.mirrors
    }

    /// Retrieve the value for the target_environments_directory field.
    pub fn detached_environments(&self) -> DetachedEnvironments {
        self.extensions
            .detached_environments
            .clone()
            .unwrap_or_default()
    }

    /// Resolve the detached-environments directory for this config, honoring
    /// this config's `[cache]` settings when the directory has to be derived.
    /// Returns `None` when detached-environments is disabled.
    pub fn detached_environments_dir(&self) -> miette::Result<Option<PathBuf>> {
        self.detached_environments().path(&self.extensions.cache)
    }

    pub fn force_activate(&self) -> bool {
        self.extensions.shell.force_activate.unwrap_or(false)
    }

    pub fn experimental_activation_cache_usage(&self) -> bool {
        self.extensions
            .experimental
            .use_environment_activation_cache()
    }

    /// Retrieve the value for the max_concurrent_solves field.
    pub fn max_concurrent_solves(&self) -> usize {
        self.concurrency.solves
    }

    /// Retrieve the value for the network_requests field.
    pub fn max_concurrent_downloads(&self) -> usize {
        self.concurrency.downloads
    }

    /// The platform to use to install tools.
    pub fn tool_platform(&self) -> Platform {
        self.extensions.tool_platform.unwrap_or(Platform::current())
    }

    pub fn get_proxies(&self) -> reqwest::Result<Vec<Proxy>> {
        if (self.proxy_config.https.is_none() && self.proxy_config.http.is_none())
            || *USE_PROXY_FROM_ENV
        {
            return Ok(vec![]);
        }

        let config_no_proxy =
            Some(self.proxy_config.non_proxy_hosts.iter().join(",")).filter(|v| !v.is_empty());

        let mut result: Vec<Proxy> = Vec::new();
        let config_no_proxy: Option<NoProxy> =
            config_no_proxy.as_deref().and_then(NoProxy::from_string);

        if self.proxy_config.https == self.proxy_config.http {
            result.push(
                Proxy::all(
                    self.proxy_config
                        .https
                        .as_ref()
                        .expect("must be some")
                        .as_str(),
                )?
                .no_proxy(config_no_proxy),
            );
        } else {
            if let Some(url) = &self.proxy_config.http {
                result.push(Proxy::http(url.as_str())?.no_proxy(config_no_proxy.clone()));
            }
            if let Some(url) = &self.proxy_config.https {
                result.push(Proxy::https(url.as_str())?.no_proxy(config_no_proxy));
            }
        }

        Ok(result)
    }

    /// Modify this config with the given key and value
    ///
    /// Keys are dotted TOML paths (`concurrency.solves`,
    /// `s3-options.my-bucket.region`). Everything except a handful of
    /// pixi-specific special cases is handled by the generic
    /// [`ConfigBase::set`], which works for both the shared keys and the
    /// pixi extension keys.
    ///
    /// # Note
    ///
    /// It is required to call `save()` to persist the changes.
    pub fn set(&mut self, key: &str, value: Option<String>) -> miette::Result<()> {
        match key {
            // Deprecated top-level keys are rejected with a pointer to their
            // `shell.*` replacements (the extension still deserializes them
            // from files for backwards compatibility).
            "change-ps1" => {
                return Err(miette::miette!(
                    "The `change-ps1` field is deprecated. Please use the `shell.change-ps1` field instead."
                ));
            }
            "force-activate" => {
                return Err(miette::miette!(
                    "The `force-activate` field is deprecated. Please use the `shell.force-activate` field instead."
                ));
            }
            // `detached-environments` accepts booleans and paths; paths get
            // `~` expanded, which the generic editor knows nothing about.
            "detached-environments" => {
                self.inner.extensions.detached_environments = value
                    .map(|v| {
                        Ok::<_, miette::Report>(match v.as_str() {
                            "true" => DetachedEnvironments::Boolean(true),
                            "false" => DetachedEnvironments::Boolean(false),
                            _ => DetachedEnvironments::Path(PathBuf::from(v)).resolve_path()?,
                        })
                    })
                    .transpose()?;
            }
            // Everything else goes through the shared generic editor.
            _ => {
                self.inner.set(key, value).into_diagnostic()?;

                // `[cache]` paths support `~` and must be absolute; re-run
                // the same post-processing as `Config::from_toml`.
                if key == "cache" || key.starts_with("cache.") {
                    self.inner.extensions.cache.expand_paths()?;
                    self.inner.extensions.cache.validate()?;
                }
            }
        }

        Ok(())
    }

    /// Save the config to the given path.
    pub fn save(&self, to: &Path) -> miette::Result<()> {
        let contents = toml_edit::ser::to_string_pretty(&self).into_diagnostic()?;
        tracing::debug!("Saving config to: {}", to.display());

        let parent = to.parent().expect("config path should have a parent");
        fs_err::create_dir_all(parent)
            .into_diagnostic()
            .wrap_err(format!(
                "failed to create directories in '{}'",
                parent.display()
            ))?;
        fs_err::write(to, contents)
            .into_diagnostic()
            .wrap_err(format!("failed to write config to '{}'", to.display()))
    }

    /// Resolve the cache directory for `kind`, applying this config's
    /// `[cache]` settings on top of the env-var / default resolution.
    pub fn cache_dir_for(&self, kind: CacheKind) -> miette::Result<PathBuf> {
        resolve_cache_kind_dir(&self.extensions.cache, kind)
    }

    /// Constructs a [`GatewayBuilder`] with preconfigured settings.
    pub fn gateway(&self) -> GatewayBuilder {
        let repodata_cache = self.cache_dir_for(CacheKind::Repodata).unwrap_or_else(|e| {
            tracing::error!("failed to determine repodata cache directory: {e}");
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("./"))
        });

        Gateway::builder()
            .with_cache_dir(repodata_cache)
            .with_channel_config(self.into())
            .with_max_concurrent_requests(self.max_concurrent_downloads())
    }

    pub fn compute_s3_config(&self) -> HashMap<String, s3_middleware::S3Config> {
        self.s3_options
            .0
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    s3_middleware::S3Config::Custom {
                        endpoint_url: v.endpoint_url.clone(),
                        region: v.region.clone(),
                        force_path_style: v.force_path_style,
                    },
                )
            })
            .collect()
    }

    /// Retrieve the value for the run_post_link_scripts field or default to
    /// false.
    pub fn run_post_link_scripts(&self) -> RunPostLinkScripts {
        self.run_post_link_scripts.clone().unwrap_or_default()
    }
}

/// Returns the path to the system-level pixi config file.
pub fn config_path_system() -> PathBuf {
    // TODO: the base_path for Windows is currently hardcoded, it should be
    // determined via the system API to support general volume label
    #[cfg(target_os = "windows")]
    let base_path = PathBuf::from("C:\\ProgramData");
    #[cfg(not(target_os = "windows"))]
    let base_path = PathBuf::from("/etc");

    base_path.join(consts::CONFIG_DIR).join(consts::CONFIG_FILE)
}

/// Returns the path(s) to the global pixi config file.
pub fn config_path_global() -> Vec<PathBuf> {
    vec![
        // On macos, add the XDG_CONFIG_HOME directory as well, although it's not a standard and
        // not set by default.
        #[cfg(target_os = "macos")]
        std::env::var("XDG_CONFIG_HOME").ok().map(|d| {
            PathBuf::from(d)
                .join(consts::CONFIG_DIR)
                .join(consts::CONFIG_FILE)
        }),
        dirs::config_dir().map(|d| d.join(consts::CONFIG_DIR).join(consts::CONFIG_FILE)),
        pixi_home().map(|d| d.join(consts::CONFIG_FILE)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use rstest::rstest;

    use super::*;

    /// Test helper: a default `Config` with the given `[cache]` section.
    fn config_with_cache(cache: CacheConfig) -> Config {
        let mut config = Config::default();
        config.extensions.cache = cache;
        config
    }

    #[test]
    fn test_config_parse() {
        // Calls get_cache_dir() via detached_environments_dir(); serialize
        // against other tests that mutate the process env.
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let toml = format!(
            r#"default-channels = ["conda-forge"]
tls-no-verify = true
tls-root-certs = "native"
detached-environments = "{}"
pinning-strategy = "no-pin"
concurrency.solves = 5
UNUSED = "unused"
        "#,
            env!("CARGO_MANIFEST_DIR").replace('\\', "\\\\").as_str()
        );
        let (config, unused) = Config::from_toml(toml.as_str(), None).unwrap();
        assert_eq!(
            config.default_channels,
            Some(vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()])
        );
        assert_eq!(config.tls_no_verify, Some(true));
        // The legacy `"native"` spelling deserializes as an alias of
        // `System` (a load-time deprecation warning still fires).
        assert_eq!(config.tls_root_certs, Some(TlsRootCerts::System));
        assert_eq!(
            config.detached_environments_dir().unwrap(),
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        );
        assert_eq!(config.max_concurrent_solves(), 5);
        assert!(unused.contains("UNUSED"));

        let toml = r"detached-environments = true";
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.detached_environments_dir().unwrap().unwrap(),
            get_cache_dir()
                .unwrap()
                .join(consts::ENVIRONMENTS_DIR)
                .as_path()
        );
    }

    #[rstest]
    #[case("semver", PinningStrategy::Semver)]
    #[case("major", PinningStrategy::Major)]
    #[case("minor", PinningStrategy::Minor)]
    #[case("exact-version", PinningStrategy::ExactVersion)]
    #[case("latest-up", PinningStrategy::LatestUp)]
    #[case("no-pin", PinningStrategy::NoPin)]
    fn test_config_parse_pinning_strategy(#[case] input: &str, #[case] expected: PinningStrategy) {
        let toml = format!("pinning-strategy = \"{input}\"");
        let (config, _) = Config::from_toml(&toml, None).unwrap();
        assert_eq!(config.pinning_strategy(), Some(expected));
    }

    /// Assert that usage of `~` in `detached_environments` is correctly expanded
    /// to the absolute path to the home directory.
    #[test]
    fn test_detached_environments_resolve_home_dir() {
        let home_dir = dirs::home_dir().expect("Failed to resolve home directory");
        let toml = r#"detached-environments = "~/my/envs""#;

        let expected_detached_envs_path = home_dir.join("my/envs");

        let (config, _) = Config::from_toml(toml, None).unwrap();
        let actual_detached_envs_path = config.detached_environments_dir().unwrap().unwrap();

        assert_eq!(actual_detached_envs_path, expected_detached_envs_path);
    }

    /// Assert that an absolute path in `detached_environments` is preserved.
    #[test]
    fn test_detached_environments_abs_path() {
        let toml = r#"detached-environments = "/home/me/envs""#;

        let (config, _) = Config::from_toml(toml, None).unwrap();
        let actual_detached_envs_path = config.detached_environments_dir().unwrap().unwrap();
        let expected_detached_envs_path = PathBuf::from("/home/me/envs");
        assert_eq!(actual_detached_envs_path, expected_detached_envs_path);
    }

    /// Assert that en error is thrown if a relative path is used in `detached_environments`.
    #[test]
    fn test_detached_environments_relative_path() {
        let toml = r#"detached-environments = "./relative_path/""#;

        let (config, _) = Config::from_toml(toml, None).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert_eq!(
            error_message,
            "The `detached-environments` path must be an absolute path: ./relative_path/"
        );
    }

    /// Assert that a boolean in `detached_environments` is preserved.
    #[test]
    fn test_detached_environments_bool() {
        let toml = r#"detached-environments = true"#;

        let (config, _) = Config::from_toml(toml, None).unwrap();

        assert_eq!(
            config.detached_environments(),
            DetachedEnvironments::Boolean(true)
        );
    }

    #[test]
    fn test_config_from_cli() {
        // Test with all CLI options enabled
        let cli = ConfigCli {
            tls_no_verify: true,
            tls_root_certs: Some(TlsRootCerts::System),
            auth_file: None,
            pypi_keyring_provider: Some(KeyringProvider::Subprocess),
            concurrent_solves: Some(8),
            concurrent_downloads: Some(100),
            run_post_link_scripts: true,
            no_symbolic_links: false,
            no_hard_links: false,
            no_ref_links: false,
            use_environment_activation_cache: true,
            pinning_strategy: Some(PinningStrategy::Semver),
        };
        let config = Config::from(cli);
        assert_eq!(config.tls_no_verify, Some(true));
        assert_eq!(config.tls_root_certs, Some(TlsRootCerts::System));
        assert_eq!(
            config.pypi_config().keyring_provider,
            Some(KeyringProvider::Subprocess)
        );
        assert_eq!(config.concurrency.solves, 8);
        assert_eq!(config.concurrency.downloads, 100);
        assert_eq!(
            config.run_post_link_scripts,
            Some(RunPostLinkScripts::Insecure)
        );
        assert_eq!(
            config.experimental().use_environment_activation_cache,
            Some(true)
        );
        assert_eq!(config.pinning_strategy(), Some(PinningStrategy::Semver));

        let cli = ConfigCli {
            tls_no_verify: false,
            tls_root_certs: None,
            auth_file: Some(PathBuf::from("path.json")),
            pypi_keyring_provider: None,
            concurrent_solves: None,
            concurrent_downloads: None,
            run_post_link_scripts: false,
            no_symbolic_links: false,
            no_hard_links: false,
            no_ref_links: false,
            use_environment_activation_cache: false,
            pinning_strategy: None,
        };

        let config = Config::from(cli);
        assert_eq!(config.tls_no_verify, None);
        assert_eq!(config.tls_root_certs, None);
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("path.json"))
        );
        assert_eq!(config.run_post_link_scripts, None);
        assert_eq!(config.experimental().use_environment_activation_cache, None);
        assert_eq!(config.pinning_strategy(), None);
    }

    #[test]
    fn test_pypi_config_parse() {
        let toml = r#"
            [pypi-config]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            keyring-provider = "subprocess"
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.pypi_config().index_url,
            Some(Url::parse("https://pypi.org/simple").unwrap())
        );
        assert_eq!(config.pypi_config().extra_index_urls.len(), 1);
        assert_eq!(
            config.pypi_config().keyring_provider,
            Some(KeyringProvider::Subprocess)
        );
    }

    #[test]
    fn test_pypi_config_allow_insecure_host() {
        let toml = r#"
            [pypi-config]
            index-url = "https://pypi.org/simple"
            extra-index-urls = ["https://pypi.org/simple2"]
            keyring-provider = "subprocess"
            allow-insecure-host = ["https://localhost:1234", "*"]
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.pypi_config().allow_insecure_host,
            vec!["https://localhost:1234", "*",]
        );
    }

    #[test]
    fn test_s3_options_parse() {
        let toml = r#"
            [s3-options.bucket1]
            endpoint-url = "https://my-s3-host"
            region = "us-east-1"
            force-path-style = false
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        let s3_options = &config.s3_options.0;
        assert_eq!(
            s3_options["bucket1"].endpoint_url,
            Url::parse("https://my-s3-host").unwrap()
        );
        assert_eq!(s3_options["bucket1"].region, "us-east-1");
        assert!(!s3_options["bucket1"].force_path_style);
    }

    #[test]
    fn test_s3_options_invalid_config() {
        let toml = r#"
            [s3-options.bucket1]
            endpoint-url = "https://my-s3-host"
            region = "us-east-1"
            # force-path-style = false
        "#;
        let result = Config::from_toml(toml, None);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("missing field `force-path-style`")
        );
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        // This depends on the system so it's hard to test.
        assert!(config.concurrency.solves > 0);
        assert_eq!(config.concurrency.downloads, 50);
    }

    #[test]
    fn test_config_merge_priority() {
        // If I set every config key, ensure that `other wins`
        let mut config = Config::default();
        let other = Config::from(ConfigBase {
            common: CommonConfig {
                default_channels: Some(vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]),
                channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir")),
                tls_no_verify: Some(true),
                tls_root_certs: Some(TlsRootCerts::System),
                concurrency: ConcurrencyConfig {
                    solves: 5,
                    ..ConcurrencyConfig::default()
                },
                authentication_override_file: Some(PathBuf::default()),
                mirrors: IndexMap::from([(
                    Url::parse("https://conda.anaconda.org/conda-forge").unwrap(),
                    Vec::default(),
                )]),
                s3_options: S3OptionsMap(IndexMap::from([(
                    "bucket1".into(),
                    S3Options {
                        endpoint_url: Url::parse("https://my-s3-host").unwrap(),
                        region: "us-east-1".to_string(),
                        force_path_style: false,
                    },
                )])),
                repodata_config: RepodataConfig {
                    default: RepodataChannelConfig {
                        disable_bzip2: Some(true),
                        disable_sharded: Some(true),
                        disable_zstd: Some(true),
                    },
                    per_channel: HashMap::from([(
                        Url::parse("https://conda.anaconda.org/conda-forge").unwrap(),
                        RepodataChannelConfig::default(),
                    )]),
                },
                run_post_link_scripts: Some(RunPostLinkScripts::Insecure),
                allow_symbolic_links: Some(true),
                allow_hard_links: Some(true),
                allow_ref_links: Some(false),
                proxy_config: ProxyConfig::default(),
                build: BuildConfig::default(),
                ..CommonConfig::default()
            },
            extensions: PixiConfig {
                detached_environments: Some(DetachedEnvironments::Path(PathBuf::from(
                    "/path/to/envs",
                ))),
                pinning_strategy: Some(PinningStrategy::NoPin),
                experimental: ExperimentalConfig {
                    use_environment_activation_cache: Some(true),
                },
                shell: ShellConfig {
                    force_activate: Some(true),
                    source_completion_scripts: None,
                    change_ps1: Some(false),
                },
                pypi_config: PyPIConfig {
                    allow_insecure_host: Vec::from(["test".to_string()]),
                    extra_index_urls: Vec::from([Url::parse(
                        "https://conda.anaconda.org/conda-forge",
                    )
                    .unwrap()]),
                    index_url: Some(Url::parse("https://conda.anaconda.org/conda-forge").unwrap()),
                    keyring_provider: Some(KeyringProvider::Subprocess),
                },
                tool_platform: None,
                cache: CacheConfig {
                    root: Some(PathBuf::from("/some/cache/root")),
                    conda_packages: Some(PathBuf::from("/shared/pkgs")),
                    pypi_mapping: Some(PathBuf::from("/local/mapping")),
                    netfs_redirect: NetfsRedirect::Always,
                    ..CacheConfig::default()
                },
                // Deprecated keys
                change_ps1: None,
                force_activate: None,
            },
            loaded_from: Vec::from([PathBuf::from_str("test").unwrap()]),
        });
        let original_other = other.clone();
        config = config.merge_config(other);
        assert_eq!(config, original_other);
    }
    #[test]
    fn test_config_merge_multiple() {
        let mut config = Config::default();
        let other = Config::from(ConfigBase {
            common: CommonConfig {
                default_channels: Some(vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]),
                channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir")),
                tls_no_verify: Some(true),
                concurrency: ConcurrencyConfig {
                    solves: 5,
                    ..ConcurrencyConfig::default()
                },
                s3_options: S3OptionsMap(IndexMap::from([
                    (
                        "bucket1".into(),
                        S3Options {
                            endpoint_url: Url::parse("https://my-s3-host").unwrap(),
                            region: "us-east-1".to_string(),
                            force_path_style: false,
                        },
                    ),
                    (
                        "bucket2".into(),
                        S3Options {
                            endpoint_url: Url::parse("https://my-s3-host").unwrap(),
                            region: "us-east-1".to_string(),
                            force_path_style: false,
                        },
                    ),
                ])),
                ..CommonConfig::default()
            },
            extensions: PixiConfig {
                detached_environments: Some(DetachedEnvironments::Path(PathBuf::from(
                    "/path/to/envs",
                ))),
                ..PixiConfig::default()
            },
            loaded_from: Vec::new(),
        });
        config = config.merge_config(other);
        assert_eq!(
            config.default_channels,
            Some(vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()])
        );
        assert_eq!(config.tls_no_verify, Some(true));
        assert_eq!(
            config.detached_environments_dir().unwrap(),
            Some(PathBuf::from("/path/to/envs"))
        );
        assert!(config.s3_options.0.contains_key("bucket1"));

        let other2 = Config::from(ConfigBase {
            common: CommonConfig {
                default_channels: Some(vec![NamedChannelOrUrl::from_str("channel").unwrap()]),
                channel_config: ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir2")),
                tls_no_verify: Some(false),
                s3_options: S3OptionsMap(IndexMap::from([(
                    "bucket2".into(),
                    S3Options {
                        endpoint_url: Url::parse("https://my-new-s3-host").unwrap(),
                        region: "us-east-1".to_string(),
                        force_path_style: false,
                    },
                )])),
                ..CommonConfig::default()
            },
            extensions: PixiConfig {
                detached_environments: Some(DetachedEnvironments::Path(PathBuf::from(
                    "/path/to/envs2",
                ))),
                ..PixiConfig::default()
            },
            loaded_from: Vec::new(),
        });

        config = config.merge_config(other2);
        assert_eq!(
            config.default_channels,
            Some(vec![NamedChannelOrUrl::from_str("channel").unwrap()])
        );
        assert_eq!(config.tls_no_verify, Some(false));
        assert_eq!(
            config.detached_environments_dir().unwrap(),
            Some(PathBuf::from("/path/to/envs2"))
        );
        assert_eq!(config.max_concurrent_solves(), 5);
        assert!(config.s3_options.0.contains_key("bucket1"));
        assert!(config.s3_options.0.contains_key("bucket2"));
        assert!(
            config.s3_options.0["bucket2"]
                .endpoint_url
                .to_string()
                .contains("my-new-s3-host")
        );

        let d = Path::new(&env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("config");

        let config_1 = Config::from_path(&d.join("config_1.toml")).unwrap();
        let mut config_2 = Config::from_path(&d.join("config_2.toml")).unwrap();
        config_2.common.channel_config =
            ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir"));
        config_2.extensions.detached_environments = Some(DetachedEnvironments::Boolean(true));

        let mut merged = config_1.clone();
        merged = merged.merge_config(config_2);
        assert!(merged.s3_options.0.contains_key("bucket1"));

        let debug = format!("{merged:#?}");
        let debug = debug.replace("\\\\", "/");
        // replace the path with a placeholder
        let debug = debug.replace(&d.to_str().unwrap().replace('\\', "/"), "path");
        insta::assert_snapshot!(debug);
    }

    #[test]
    fn test_parse_kebab_and_snake_case() {
        let toml = r#"
            default_channels = ["conda-forge"]
            change_ps1 = true
            tls_no_verify = false
            authentication_override_file = "/path/to/your/override.json"
            [mirrors]
            "https://conda.anaconda.org/conda-forge" = [
                "https://prefix.dev/conda-forge"
            ]
            [repodata_config]
            disable_bzip2 = true
            disable_zstd = true
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.default_channels,
            Some(vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()])
        );
        assert_eq!(config.tls_no_verify, Some(false));
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("/path/to/your/override.json"))
        );
        assert_eq!(config.extensions.change_ps1, Some(true));
        assert_eq!(
            config
                .mirrors
                .get(&Url::parse("https://conda.anaconda.org/conda-forge").unwrap()),
            Some(&vec![Url::parse("https://prefix.dev/conda-forge").unwrap()])
        );
        let repodata_config = &config.repodata_config;
        assert_eq!(repodata_config.default.disable_bzip2, Some(true));
        assert_eq!(repodata_config.default.disable_zstd, Some(true));
        assert_eq!(repodata_config.default.disable_sharded, None);
        // See if the toml parses in kebab-case
        let toml = r#"
            default-channels = ["conda-forge"]
            change-ps1 = true
            tls-no-verify = false
            authentication-override-file = "/path/to/your/override.json"
            [mirrors]
            "https://conda.anaconda.org/conda-forge" = [
                "https://prefix.dev/conda-forge"
            ]
            [repodata-config]
            disable-jlap = true
            disable-bzip2 = true
            disable-zstd = true
            disable-sharded = true
        "#;
        Config::from_toml(toml, None).unwrap();
    }

    #[test]
    fn test_alter_config() {
        // Calls get_cache_dir() / cache_dir_for(), which read PIXI_CACHE_DIR;
        // serialize against other tests that mutate the process env.
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let mut config = Config::default();
        config
            .set("default-channels", Some(r#"["conda-forge"]"#.to_string()))
            .unwrap();
        assert_eq!(
            config.default_channels,
            Some(vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()])
        );

        config
            .set("tls-no-verify", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.tls_no_verify, Some(true));

        config
            .set(
                "authentication-override-file",
                Some("/path/to/your/override.json".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.authentication_override_file,
            Some(PathBuf::from("/path/to/your/override.json"))
        );

        config
            .set("detached-environments", Some("true".to_string()))
            .unwrap();
        assert_eq!(
            config.detached_environments_dir().unwrap().unwrap(),
            get_cache_dir()
                .unwrap()
                .join(consts::ENVIRONMENTS_DIR)
                .as_path()
        );

        config
            .set("detached-environments", Some("/path/to/envs".to_string()))
            .unwrap();
        assert_eq!(
            config.detached_environments_dir().unwrap(),
            Some(PathBuf::from("/path/to/envs"))
        );

        config
            .set("mirrors", Some(r#"{"https://conda.anaconda.org/conda-forge": ["https://prefix.dev/conda-forge"]}"#.to_string()))
            .unwrap();
        assert_eq!(
            config
                .mirrors
                .get(&Url::parse("https://conda.anaconda.org/conda-forge").unwrap()),
            Some(&vec![Url::parse("https://prefix.dev/conda-forge").unwrap()])
        );

        config
            .set(
                "pypi-config.index-url",
                Some("https://pypi.org/simple".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.pypi_config().index_url,
            Some(Url::parse("https://pypi.org/simple").unwrap())
        );

        config
            .set(
                "pypi-config.extra-index-urls",
                Some(r#"["https://pypi.org/simple2"]"#.to_string()),
            )
            .unwrap();
        assert!(config.pypi_config().extra_index_urls.len() == 1);

        config
            .set(
                "pypi-config.keyring-provider",
                Some("subprocess".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.pypi_config().keyring_provider,
            Some(KeyringProvider::Subprocess)
        );

        let deprecated = config.set("change-ps1", None);
        assert!(deprecated.is_err());
        assert!(deprecated.unwrap_err().to_string().contains("deprecated"));

        config.set("shell.change-ps1", None).unwrap();
        assert_eq!(config.extensions.change_ps1, None);

        config
            .set("concurrency.solves", Some("10".to_string()))
            .unwrap();
        assert_eq!(config.max_concurrent_solves(), 10);
        config
            .set("concurrency.solves", Some("1".to_string()))
            .unwrap();

        config
            .set("concurrency.downloads", Some("10".to_string()))
            .unwrap();
        assert_eq!(config.max_concurrent_downloads(), 10);
        config
            .set("concurrency.downloads", Some("1".to_string()))
            .unwrap();

        assert_eq!(config.max_concurrent_downloads(), 1);

        config.set("s3-options.my-bucket", Some(r#"{"endpoint-url": "http://localhost:9000", "force-path-style": true, "region": "auto"}"#.to_string())).unwrap();
        let s3_options = config.s3_options.0.get("my-bucket").unwrap();
        assert!(
            s3_options
                .endpoint_url
                .to_string()
                .contains("http://localhost:9000")
        );
        assert!(s3_options.force_path_style);
        assert_eq!(s3_options.region, "auto");

        // Test tool-platform
        config
            .set("tool-platform", Some("linux-64".to_string()))
            .unwrap();
        assert_eq!(config.extensions.tool_platform, Some(Platform::Linux64));

        // Test run-post-link-scripts
        config
            .set("run-post-link-scripts", Some("insecure".to_string()))
            .unwrap();
        assert_eq!(
            config.run_post_link_scripts,
            Some(RunPostLinkScripts::Insecure)
        );

        // Test shell.force-activate
        config
            .set("shell.force-activate", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.extensions.shell.force_activate, Some(true));

        // Test shell.source-completion-scripts
        config
            .set("shell.source-completion-scripts", Some("false".to_string()))
            .unwrap();
        assert_eq!(
            config.extensions.shell.source_completion_scripts,
            Some(false)
        );

        // Test experimental.use-environment-activation-cache
        config
            .set(
                "experimental.use-environment-activation-cache",
                Some("true".to_string()),
            )
            .unwrap();
        assert_eq!(
            config
                .extensions
                .experimental
                .use_environment_activation_cache,
            Some(true)
        );

        // Test more repodata-config options
        // disable-jlap has been removed — setting it should error
        assert!(
            config
                .set("repodata-config.disable-jlap", Some("true".to_string()))
                .is_err()
        );

        config
            .set("repodata-config.disable-bzip2", Some("true".to_string()))
            .unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_bzip2, Some(true));

        config
            .set("repodata-config.disable-zstd", Some("false".to_string()))
            .unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_zstd, Some(false));

        config
            .set("repodata-config.disable-sharded", Some("true".to_string()))
            .unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_sharded, Some(true));

        // Test pypi-config.allow-insecure-host
        config
            .set(
                "pypi-config.allow-insecure-host",
                Some(r#"["pypi.example.com"]"#.to_string()),
            )
            .unwrap();
        assert_eq!(config.pypi_config().allow_insecure_host.len(), 1);

        // Test proxy-config
        config
            .set(
                "proxy-config.http",
                Some("http://proxy.example.com:8080".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.proxy_config.http,
            Some(Url::parse("http://proxy.example.com:8080").unwrap())
        );

        config
            .set(
                "proxy-config.https",
                Some("https://proxy.example.com:8080".to_string()),
            )
            .unwrap();
        assert_eq!(
            config.proxy_config.https,
            Some(Url::parse("https://proxy.example.com:8080").unwrap())
        );

        config
            .set(
                "proxy-config.non-proxy-hosts",
                Some(r#"["localhost", "127.0.0.1"]"#.to_string()),
            )
            .unwrap();
        assert_eq!(config.proxy_config.non_proxy_hosts.len(), 2);

        // Test s3-options with individual keys. The bucket has to exist
        // before its individual fields can be edited: a partial bucket
        // table is rejected (`S3Options` requires all three fields).
        // Note: editing a *non-existent* bucket's subkey used to be a
        // silent no-op; with the generic editor it is now a proper error.
        assert!(
            config
                .set(
                    "s3-options.absent-bucket.region",
                    Some("us-east-1".to_string()),
                )
                .is_err()
        );
        config.set("s3-options.test-bucket", Some(r#"{"endpoint-url": "http://localhost:2222", "force-path-style": true, "region": "eu-west-1"}"#.to_string())).unwrap();
        config
            .set(
                "s3-options.test-bucket.endpoint-url",
                Some("http://localhost:9000".to_string()),
            )
            .unwrap();
        config
            .set(
                "s3-options.test-bucket.region",
                Some("us-east-1".to_string()),
            )
            .unwrap();
        config
            .set(
                "s3-options.test-bucket.force-path-style",
                Some("false".to_string()),
            )
            .unwrap();
        let test_bucket = &config.s3_options.0["test-bucket"];
        assert_eq!(
            test_bucket.endpoint_url,
            Url::parse("http://localhost:9000").unwrap()
        );
        assert_eq!(test_bucket.region, "us-east-1");
        assert!(!test_bucket.force_path_style);

        // Test concurrency configuration
        config
            .set("concurrency.solves", Some("5".to_string()))
            .unwrap();
        assert_eq!(config.concurrency.solves, 5);

        config
            .set("concurrency.downloads", Some("25".to_string()))
            .unwrap();
        assert_eq!(config.concurrency.downloads, 25);

        // Test max-concurrent-solves (legacy accessor)
        assert_eq!(config.max_concurrent_solves(), 5);
        assert_eq!(config.max_concurrent_downloads(), 25);

        // Test tls-no-verify
        config
            .set("tls-no-verify", Some("true".to_string()))
            .unwrap();
        assert_eq!(config.tls_no_verify, Some(true));

        // Test tls-root-certs
        config
            .set("tls-root-certs", Some("system".to_string()))
            .unwrap();
        assert_eq!(config.tls_root_certs, Some(TlsRootCerts::System));

        // The deprecated `native` spelling still deserializes (as an alias
        // of `System`).
        config
            .set("tls-root-certs", Some("native".to_string()))
            .unwrap();
        assert_eq!(config.tls_root_certs, Some(TlsRootCerts::System));

        // Test mirrors
        config
            .set(
                "mirrors",
                Some(r#"{"https://conda.anaconda.org/conda-forge": ["https://prefix.dev/conda-forge"]}"#.to_string()),
            )
            .unwrap();
        assert_eq!(config.mirrors.len(), 1);

        // Test detached-environments
        config
            .set("detached-environments", Some("/custom/path".to_string()))
            .unwrap();
        assert!(matches!(
            config.extensions.detached_environments,
            Some(DetachedEnvironments::Path(_))
        ));

        // Test pinning-strategy
        config
            .set("pinning-strategy", Some("semver".to_string()))
            .unwrap();
        assert_eq!(
            config.extensions.pinning_strategy,
            Some(PinningStrategy::Semver)
        );

        config.set("unknown-key", None).unwrap_err();
    }

    #[rstest]
    #[case("pinning-strategy", None, None)]
    #[case("pinning-strategy", Some("semver".to_string()), Some(PinningStrategy::Semver))]
    #[case("pinning-strategy", Some("no-pin".to_string()), Some(PinningStrategy::NoPin))]
    #[case("pinning-strategy", Some("exact-version".to_string()), Some(PinningStrategy::ExactVersion))]
    #[case("pinning-strategy", Some("latest-up".to_string()), Some(PinningStrategy::LatestUp))]
    #[case("pinning-strategy", Some("major".to_string()), Some(PinningStrategy::Major))]
    #[case("pinning-strategy", Some("minor".to_string()), Some(PinningStrategy::Minor))]
    fn test_set_pinning_strategy(
        #[case] key: &str,
        #[case] value: Option<String>,
        #[case] expected: Option<PinningStrategy>,
    ) {
        let mut config = Config::default();
        config.set(key, value).unwrap();
        assert_eq!(config.extensions.pinning_strategy, expected);
    }

    #[test]
    fn test_version_constraints() {
        let versions = vec![
            vec!["1.2.3"],
            vec!["0.0.0"],
            vec!["1!1"],
            vec!["1!0.0.0"],
            vec!["1.2.3a1"],
            vec!["1.2.3a1", "1.2.3"],
            vec!["1.2.3", "1.2.3a1", "1.2.3b1", "1.2.3rc1", "2", "10000"],
            vec!["1.2.0", "1.3.0"],
            vec!["0.2.0", "0.3.0"],
            vec!["0.2.0", "1.3.0"],
            vec!["1.2"],
            vec!["1.2", "2"],
            vec!["1.2", "1!2.0"],
            vec!["24.2"],
        ];

        // We could use `strum` for this, but it requires another dependency
        let strategies = vec![
            PinningStrategy::Semver,
            PinningStrategy::Major,
            PinningStrategy::Minor,
            PinningStrategy::ExactVersion,
            PinningStrategy::LatestUp,
            PinningStrategy::NoPin,
        ];

        let results = strategies
            .into_iter()
            .map(|strategy| {
                let constraints: Vec<String> = versions
                    .iter()
                    .map(|v| {
                        let constraint = strategy
                            .determine_version_constraint(
                                v.clone()
                                    .into_iter()
                                    .map(|v| v.parse().unwrap())
                                    .collect::<Vec<Version>>()
                                    .as_slice(),
                            )
                            .unwrap()
                            .to_string();
                        format!("{} from {}", constraint, v.join(", "))
                    })
                    .collect();
                format!(
                    "### Strategy: '{:?}'\n{}\n",
                    strategy,
                    constraints.join("\n")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(results);
    }

    #[test]
    fn test_repodata_config() {
        let toml = r#"
            [repodata-config]
            disable-bzip2 = true
            disable-zstd = true
            disable-sharded = true

            [repodata-config."https://prefix.dev/conda-forge"]
            disable-bzip2 = false
            disable-zstd = false
            disable-sharded = false

            [repodata-config."https://conda.anaconda.org/conda-forge"]
            disable-bzip2 = false
            disable-zstd = false
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        let repodata_config = config.repodata_config();
        assert_eq!(repodata_config.default.disable_bzip2, Some(true));
        assert_eq!(repodata_config.default.disable_zstd, Some(true));
        assert_eq!(repodata_config.default.disable_sharded, Some(true));

        let per_channel = repodata_config.clone().per_channel;
        assert_eq!(per_channel.len(), 2);

        let prefix_config = per_channel
            .get(&Url::from_str("https://prefix.dev/conda-forge").unwrap())
            .unwrap();
        assert_eq!(prefix_config.disable_bzip2, Some(false));
        assert_eq!(prefix_config.disable_zstd, Some(false));
        assert_eq!(prefix_config.disable_sharded, Some(false));

        let anaconda_config = per_channel
            .get(&Url::from_str("https://conda.anaconda.org/conda-forge").unwrap())
            .unwrap();
        assert_eq!(anaconda_config.disable_bzip2, Some(false));
        assert_eq!(anaconda_config.disable_zstd, Some(false));
        assert_eq!(anaconda_config.disable_sharded, None);
    }

    #[test]
    fn test_proxy_config_parse() {
        let toml = r#"
            [proxy-config]
            https = "http://proxy-for-https"
            http = "http://proxy-for-http"
            non-proxy-hosts = [ "a.com" ]
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.proxy_config.https,
            Some(Url::parse("http://proxy-for-https").unwrap())
        );
        assert_eq!(
            config.proxy_config.http,
            Some(Url::parse("http://proxy-for-http").unwrap())
        );
        assert_eq!(config.proxy_config.non_proxy_hosts.len(), 1);
        assert_eq!(config.proxy_config.non_proxy_hosts[0], "a.com");
    }

    use std::str::FromStr;

    use super::PackageFormatAndCompression;

    // We exercise the `FromStr` parse path and compare via the canonical
    // `archive:level` string emitted by the type's `Serialize` impl.
    fn parsed_as(input: &str) -> String {
        let p = PackageFormatAndCompression::from_str(input).unwrap();
        serde_json::to_string(&p).unwrap()
    }

    #[test]
    fn test_parse_packaging() {
        assert_eq!(parsed_as("tar-bz2"), "\"tarbz2:default\"");
        assert_eq!(parsed_as("conda"), "\"conda:default\"");
        assert_eq!(parsed_as("tar-bz2:1"), "\"tarbz2:1\"");
        assert_eq!(parsed_as(".tar.bz2:max"), "\"tarbz2:max\"");
        assert_eq!(parsed_as("tarbz2:5"), "\"tarbz2:5\"");
        assert_eq!(parsed_as("conda:1"), "\"conda:1\"");
        assert_eq!(parsed_as("conda:max"), "\"conda:max\"");
        assert_eq!(parsed_as("conda:-5"), "\"conda:-5\"");
        assert_eq!(parsed_as("conda:fast"), "\"conda:min\"");
    }

    // Serialize env-var-sensitive tests so they don't race against each other.
    // Parallel cargo-test threads share the process environment, and
    // cache_dir_for / is_network_filesystem read several env vars.
    static NETFS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct ScopedEnv {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl ScopedEnv {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: tests take NETFS_ENV_LOCK before touching the process env,
            // so no other thread is reading/writing env simultaneously.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: see `set` above.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: see `ScopedEnv::set`.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn netfs_force_redirect_env_wins() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");
        assert!(is_network_filesystem(&PathBuf::from("/tmp")));
    }

    #[test]
    fn netfs_disable_beats_force() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _disable = ScopedEnv::set("PIXI_DISABLE_NETFS_REDIRECT", "1");
        assert!(!is_network_filesystem(&PathBuf::from("/tmp")));
    }

    #[test]
    fn netfs_local_tempdir_is_not_network() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::unset("PIXI_FORCE_NETFS_REDIRECT");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");
        // CI runners and local dev both have /tmp (or macOS equivalent) on
        // local disk. If this ever fails the test runner is itself on NFS.
        assert!(!is_network_filesystem(&std::env::temp_dir()));
    }

    #[test]
    fn cache_dir_for_honors_user_pinned_cache_dir() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        // Even with force-redirect set, an explicit user pin must win.
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _pin = ScopedEnv::set("PIXI_CACHE_DIR", "/some/user/path");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");

        let got = Config::default()
            .cache_dir_for(CacheKind::PypiMapping)
            .unwrap();
        assert_eq!(got, PathBuf::from("/some/user/path/conda-pypi-mapping"));
    }

    #[test]
    fn cache_dir_for_redirects_local_kinds_when_network_detected() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _cache = ScopedEnv::unset("PIXI_CACHE_DIR");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");

        let got = Config::default()
            .cache_dir_for(CacheKind::PypiMapping)
            .unwrap();

        assert!(got.ends_with("conda-pypi-mapping"));
        assert!(got.starts_with(node_local_scratch_dir()));
    }

    #[test]
    fn cache_dir_for_keeps_shared_kinds_on_netfs() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _cache = ScopedEnv::unset("PIXI_CACHE_DIR");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");

        // The conda package cache benefits from being on a shared filesystem;
        // it should not be redirected to node-local scratch even when the
        // root looks like netfs.
        let got = Config::default()
            .cache_dir_for(CacheKind::CondaPackages)
            .unwrap();
        assert!(got.ends_with(consts::CONDA_PACKAGE_CACHE_DIR));
        assert!(!got.starts_with(node_local_scratch_dir()));
    }

    #[test]
    fn cache_dir_for_passes_through_when_local() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::unset("PIXI_FORCE_NETFS_REDIRECT");
        let _cache = ScopedEnv::unset("PIXI_CACHE_DIR");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");

        let got = Config::default()
            .cache_dir_for(CacheKind::PypiMapping)
            .unwrap();
        let expected = get_cache_dir().unwrap().join("conda-pypi-mapping");
        assert_eq!(got, expected);
    }

    #[test]
    fn cache_kind_config_key_matches_toml_field() {
        // Regression for #6281: the netfs-redirect warning suggests
        // `[cache.<config_key>]`, which must be a key the parser actually
        // accepts. Round-trip each kind's config_key through TOML and assert
        // it populates that kind's per-kind path.
        let kinds = [
            CacheKind::CondaPackages,
            CacheKind::Repodata,
            CacheKind::PypiWheels,
            CacheKind::PypiMapping,
            CacheKind::ExecEnvironments,
            CacheKind::BuildToolEnvironments,
            CacheKind::DetachedEnvironments,
        ];
        for kind in kinds {
            let toml = format!("{} = \"/abs/path\"\n", kind.config_key());
            let cache: CacheConfig = toml_edit::de::from_str(&toml)
                .unwrap_or_else(|e| panic!("'{}' did not parse: {e}", kind.config_key()));
            assert_eq!(
                cache.path_for(kind),
                Some(Path::new("/abs/path")),
                "config_key '{}' did not populate the path for {:?}",
                kind.config_key(),
                kind,
            );
        }
    }

    #[test]
    fn cache_dir_for_per_kind_path_override_wins() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        // Even when redirection is forced and a config-level root is set,
        // the per-kind [cache.pypi-mapping] path should override.
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");

        let config = config_with_cache(CacheConfig {
            root: Some(PathBuf::from("/some/configured/root")),
            pypi_mapping: Some(PathBuf::from("/explicit/per/kind/path")),
            ..CacheConfig::default()
        });

        let got = config.cache_dir_for(CacheKind::PypiMapping).unwrap();
        assert_eq!(got, PathBuf::from("/explicit/per/kind/path"));
    }

    #[test]
    fn cache_dir_for_workspace_pypi_mapping_override_wins() {
        // Regression test for #6281: a workspace-level `[cache.pypi-mapping]`
        // override (merged on top of the global config) must be honored when
        // resolving the conda-pypi mapping cache path. Previously the mapping
        // client resolved through a global-only path and silently ignored the
        // workspace override, so the netfs redirect kept firing.
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        // Force the redirect so that, without the override, the path would be
        // rewritten to node-local scratch and the warning would fire.
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");

        // The global (system + user) layer has no per-kind override.
        let global = Config::default();
        // The workspace `.pixi/config.toml` sets the mapping cache path.
        let workspace = config_with_cache(CacheConfig {
            pypi_mapping: Some(PathBuf::from("/workspace/conda-pypi-mapping")),
            ..CacheConfig::default()
        });

        // `merge_config` mirrors how the workspace config is layered onto the
        // global config; this merged `Config` is what the mapping client now
        // consults.
        let merged = global.merge_config(workspace);

        let got = merged.cache_dir_for(CacheKind::PypiMapping).unwrap();
        assert_eq!(got, PathBuf::from("/workspace/conda-pypi-mapping"));
        // And crucially the path is *not* redirected to node-local scratch.
        assert!(!got.starts_with(node_local_scratch_dir()));
    }

    #[test]
    fn cache_dir_for_config_root_acts_like_user_pin() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _cache = ScopedEnv::unset("PIXI_CACHE_DIR");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");

        // [cache.root] in config should behave like a user pin: no redirect
        // even with FORCE_NETFS_REDIRECT, because the user explicitly chose
        // this location.
        let config = config_with_cache(CacheConfig {
            root: Some(PathBuf::from("/configured/root")),
            ..CacheConfig::default()
        });

        let got = config.cache_dir_for(CacheKind::PypiMapping).unwrap();
        assert_eq!(got, PathBuf::from("/configured/root/conda-pypi-mapping"));
    }

    #[test]
    fn cache_dir_for_netfs_redirect_never_disables_redirect() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _cache = ScopedEnv::unset("PIXI_CACHE_DIR");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");

        let config = config_with_cache(CacheConfig {
            netfs_redirect: NetfsRedirect::Never,
            ..CacheConfig::default()
        });

        let got = config.cache_dir_for(CacheKind::PypiMapping).unwrap();
        assert!(!got.starts_with(node_local_scratch_dir()));
    }

    #[test]
    fn cache_config_parses_from_toml() {
        let toml = r#"
            [cache]
            root = "/shared/hpc"
            conda-packages = "/shared/hpc/pkgs"
            pypi-mapping = "/local/scratch/mapping"
            netfs-redirect = "never"
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.extensions.cache.root,
            Some(PathBuf::from("/shared/hpc"))
        );
        assert_eq!(
            config.extensions.cache.conda_packages,
            Some(PathBuf::from("/shared/hpc/pkgs"))
        );
        assert_eq!(
            config.extensions.cache.pypi_mapping,
            Some(PathBuf::from("/local/scratch/mapping"))
        );
        assert_eq!(config.extensions.cache.netfs_redirect, NetfsRedirect::Never);
    }

    #[test]
    fn cache_config_expands_tilde_in_paths() {
        let home_dir = dirs::home_dir().expect("home dir resolves on test host");
        let toml = r#"
            [cache]
            root = "~/.cache/pixi"
            pypi-mapping = "~/scratch/mapping"
        "#;
        let (config, _) = Config::from_toml(toml, None).unwrap();
        assert_eq!(
            config.extensions.cache.root,
            Some(home_dir.join(".cache/pixi"))
        );
        assert_eq!(
            config.extensions.cache.pypi_mapping,
            Some(home_dir.join("scratch/mapping"))
        );
    }

    #[test]
    fn cache_config_rejects_relative_paths_on_validate() {
        let cfg = CacheConfig {
            pypi_mapping: Some(PathBuf::from("not-absolute")),
            ..CacheConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("cache.pypi-mapping"),
            "error should name the offending field, got: {msg}"
        );
        assert!(
            msg.contains("must be an absolute path"),
            "error should explain the requirement, got: {msg}"
        );
    }

    #[test]
    fn cache_config_validate_passes_when_unset() {
        // An empty CacheConfig is valid (no paths to check).
        CacheConfig::default().validate().unwrap();
    }

    #[test]
    fn cache_dir_for_per_kind_env_overrides_toml() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        // Even with a TOML per-kind path AND force-redirect set, the env var
        // wins and bypasses redirect.
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");
        let _env = ScopedEnv::set("PIXI_CACHE_PYPI_MAPPING_DIR", "/from/env/mapping");

        let config = config_with_cache(CacheConfig {
            pypi_mapping: Some(PathBuf::from("/from/toml/mapping")),
            ..CacheConfig::default()
        });

        let got = config.cache_dir_for(CacheKind::PypiMapping).unwrap();
        assert_eq!(got, PathBuf::from("/from/env/mapping"));
    }

    #[test]
    fn cache_dir_for_per_kind_env_only_affects_named_kind() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::unset("PIXI_FORCE_NETFS_REDIRECT");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");
        let _cache = ScopedEnv::set("PIXI_CACHE_DIR", "/pinned/root");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");
        let _env = ScopedEnv::set("PIXI_CACHE_REPODATA_DIR", "/from/env/repodata");

        // Repodata uses the env-var path verbatim.
        let got_repo = Config::default()
            .cache_dir_for(CacheKind::Repodata)
            .unwrap();
        assert_eq!(got_repo, PathBuf::from("/from/env/repodata"));

        // Other kinds keep their default subdir under the pinned root.
        let got_wheels = Config::default()
            .cache_dir_for(CacheKind::PypiWheels)
            .unwrap();
        assert!(got_wheels.starts_with("/pinned/root"));
        assert!(!got_wheels.starts_with("/from/env"));
    }

    #[test]
    fn cache_dir_for_netfs_redirect_env_overrides_toml() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        // TOML says `always`, env says `never` — env wins, so no redirect.
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _cache = ScopedEnv::unset("PIXI_CACHE_DIR");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");
        let _redirect = ScopedEnv::set("PIXI_CACHE_NETFS_REDIRECT", "never");

        let config = config_with_cache(CacheConfig {
            netfs_redirect: NetfsRedirect::Always,
            ..CacheConfig::default()
        });

        let got = config.cache_dir_for(CacheKind::PypiMapping).unwrap();
        assert!(!got.starts_with(node_local_scratch_dir()));
    }

    #[test]
    fn cache_dir_for_netfs_redirect_env_invalid_falls_back_to_toml() {
        let _guard = NETFS_ENV_LOCK.lock().unwrap();
        let _force = ScopedEnv::set("PIXI_FORCE_NETFS_REDIRECT", "1");
        let _cache = ScopedEnv::unset("PIXI_CACHE_DIR");
        let _rattler = ScopedEnv::unset("RATTLER_CACHE_DIR");
        let _disable = ScopedEnv::unset("PIXI_DISABLE_NETFS_REDIRECT");
        let _redirect = ScopedEnv::set("PIXI_CACHE_NETFS_REDIRECT", "garbage");

        let config = config_with_cache(CacheConfig {
            netfs_redirect: NetfsRedirect::Never,
            ..CacheConfig::default()
        });

        // Bad env value → ignored, TOML `never` still applies.
        let got = config.cache_dir_for(CacheKind::PypiMapping).unwrap();
        assert!(!got.starts_with(node_local_scratch_dir()));
    }

    #[test]
    fn config_validate_surfaces_cache_errors() {
        // Config::validate must propagate CacheConfig::validate failures so
        // bad cache paths are caught at load time, not at first cache use.
        let config = config_with_cache(CacheConfig {
            conda_packages: Some(PathBuf::from("relative/dir")),
            ..CacheConfig::default()
        });
        let err = config.validate().unwrap_err();
        assert!(format!("{err}").contains("cache.conda-packages"));
    }

    /// Extension keys (pixi-specific keys layered on the shared rattler
    /// config) are editable through the upstream *generic*
    /// `ConfigBase::set` — no pixi-specific `match` arm involved.
    #[test]
    fn test_generic_set_handles_extension_keys() {
        let mut config = Config::default();

        // A top-level extension key ...
        config
            .set("pinning-strategy", Some("semver".to_string()))
            .unwrap();
        assert_eq!(
            config.extensions.pinning_strategy,
            Some(PinningStrategy::Semver)
        );

        // ... a nested extension key ...
        config
            .set("shell.change-ps1", Some("false".to_string()))
            .unwrap();
        assert_eq!(config.extensions.shell.change_ps1, Some(false));

        // ... and unsetting works too.
        config.set("pinning-strategy", None).unwrap();
        assert_eq!(config.extensions.pinning_strategy, None);

        // A typo in an extension key is rejected, and the error lists the
        // supported keys — including both shared and extension keys.
        let err = config
            .set("pinning-strateggy", Some("semver".to_string()))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("pinning-strateggy"), "got: {msg}");
        assert!(msg.contains("pinning-strategy"), "got: {msg}");
        assert!(msg.contains("default-channels"), "got: {msg}");
        // ... and the config was not modified.
        assert_eq!(config.extensions.pinning_strategy, None);
    }

    /// Unknown keys — including typos of extension keys and unknown keys
    /// nested inside known extension tables — surface from `from_toml` so
    /// the "Ignoring '…' in file" warnings keep working.
    #[test]
    fn test_from_toml_reports_unknown_extension_keys() {
        let toml = r#"
            default-channels = ["conda-forge"]
            pinning-strategy = "semver"
            pinning-strateggy = "semver"

            [shell]
            change-ps1 = true
            chnge-ps1 = true

            [pypi-config]
            index-url = "https://pypi.org/simple"
            index-urll = "https://pypi.org/simple"
        "#;
        let (config, unused) = Config::from_toml(toml, None).unwrap();

        // Valid keys were consumed by the common config or the extension.
        assert_eq!(
            config.extensions.pinning_strategy,
            Some(PinningStrategy::Semver)
        );
        assert_eq!(config.extensions.shell.change_ps1, Some(true));
        assert_eq!(
            config.extensions.pypi_config.index_url,
            Some(Url::parse("https://pypi.org/simple").unwrap())
        );

        // The typos are reported, at full depth.
        assert!(unused.contains("pinning-strateggy"), "got: {unused:?}");
        assert!(unused.contains("shell.chnge-ps1"), "got: {unused:?}");
        assert!(unused.contains("pypi-config.index-urll"), "got: {unused:?}");

        // Consumed keys are not reported.
        assert!(!unused.contains("pinning-strategy"));
        assert!(!unused.iter().any(|k| k == "shell" || k == "pypi-config"));
    }
}
