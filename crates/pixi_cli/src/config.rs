use crate::cli_config::WorkspaceConfig;
use clap::Parser;
use miette::{IntoDiagnostic, WrapErr};
use pixi_config;
use pixi_config::{Config, ConfigError, GlobalConfigSource};
use pixi_consts::consts;
use pixi_core::WorkspaceLocator;
use pixi_core::workspace::WorkspaceLocatorError;
use rattler_conda_types::NamedChannelOrUrl;
use std::{io::Write, path::PathBuf, str::FromStr};

#[derive(Parser, Debug)]
enum Subcommand {
    /// Edit the configuration file
    #[clap(alias = "e")]
    Edit(EditArgs),

    /// List configuration values
    ///
    /// Example: `pixi config list default-channels`
    #[clap(visible_alias = "ls", alias = "l")]
    List(ListArgs),

    /// Prepend a value to a list configuration key
    ///
    /// Example: `pixi config prepend default-channels bioconda`
    Prepend(PendArgs),

    /// Append a value to a list configuration key
    ///
    /// Example: `pixi config append default-channels bioconda`
    Append(PendArgs),

    /// Set a configuration value
    ///
    /// Example: `pixi config set default-channels '["conda-forge", "bioconda"]'`
    Set(SetArgs),

    /// Unset a configuration value
    ///
    /// Example: `pixi config unset default-channels`
    Unset(UnsetArgs),
}

#[derive(Parser, Debug, Clone)]
struct CommonArgs {
    /// Operation on project-local configuration
    #[arg(long, short, conflicts_with_all = &["global", "system"], help_heading = consts::CLAP_CONFIG_OPTIONS)]
    local: bool,

    /// Operation on global configuration
    #[arg(long, short, conflicts_with_all = &["local", "system"], help_heading = consts::CLAP_CONFIG_OPTIONS)]
    global: bool,

    /// Operation on system configuration
    #[arg(long, short, conflicts_with_all = &["local", "global"], help_heading = consts::CLAP_CONFIG_OPTIONS)]
    system: bool,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,
}

#[derive(Parser, Debug, Clone)]
struct EditArgs {
    #[clap(flatten)]
    common: CommonArgs,

    /// The editor to use, defaults to `EDITOR` environment variable or `nano` on Unix and `notepad` on Windows
    #[arg(env = "EDITOR")]
    pub editor: Option<String>,
}

#[derive(Parser, Debug, Clone)]
struct ListArgs {
    /// Configuration key to show (all if not provided)
    key: Option<String>,

    /// Output in JSON format
    #[arg(long)]
    json: bool,

    #[clap(flatten)]
    config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct PendArgs {
    /// Configuration key to set
    key: String,

    /// Configuration value to (pre|ap)pend
    value: String,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct SetArgs {
    /// Configuration key to set
    key: String,

    /// Configuration value to set (key will be unset if value not provided)
    value: Option<String>,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct UnsetArgs {
    /// Configuration key to unset
    key: String,

    #[clap(flatten)]
    common: CommonArgs,
}

enum AlterMode {
    Prepend,
    Append,
    Set,
    Unset,
}

/// Configuration management
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    #[clap(subcommand)]
    subcommand: Subcommand,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.subcommand {
        Subcommand::Edit(args) => {
            let config_path = determine_config_write_path(&args.common)?;

            let editor = args.editor.unwrap_or_else(|| {
                if cfg!(windows) {
                    "notepad".to_string()
                } else {
                    "nano".to_string()
                }
            });

            let mut child = if cfg!(windows) {
                std::process::Command::new("cmd")
                    .arg("/C")
                    .arg(editor.as_str())
                    .arg(&config_path)
                    .spawn()
                    .into_diagnostic()?
            } else {
                std::process::Command::new(editor.as_str())
                    .arg(&config_path)
                    .spawn()
                    .into_diagnostic()?
            };
            child.wait().into_diagnostic()?;
        }
        Subcommand::List(args) => {
            let mut config = load_config(&args.common, &args.config_source.source())?;

            if let Some(key) = &args.key {
                partial_config(&mut config, key)?;
            }

            let mut out = if args.json {
                serde_json::to_string_pretty(&config).into_diagnostic()?
            } else {
                toml_edit::ser::to_string_pretty(&config).into_diagnostic()?
            };

            if let Some(key) = &args.key {
                prune_subkeys(&mut out, key, args.json)?;
            }

            if out.trim().is_empty() {
                eprintln!("Configuration not set");
            } else {
                pixi_utils::io::ignore_broken_pipe(writeln!(std::io::stdout(), "{out}"))
                    .into_diagnostic()?;
            }
        }
        Subcommand::Prepend(args) => alter_config(
            &args.common,
            &args.key,
            Some(args.value),
            AlterMode::Prepend,
        )?,
        Subcommand::Append(args) => {
            alter_config(&args.common, &args.key, Some(args.value), AlterMode::Append)?
        }
        Subcommand::Set(args) => alter_config(&args.common, &args.key, args.value, AlterMode::Set)?,
        Subcommand::Unset(args) => alter_config(&args.common, &args.key, None, AlterMode::Unset)?,
    };
    Ok(())
}

fn determine_project_root(common_args: &CommonArgs) -> miette::Result<Option<PathBuf>> {
    let workspace = WorkspaceLocator::default()
        .with_closest_package(false) // Dont care about the package
        .with_emit_warnings(false) // No reason to emit warnings
        .with_consider_environment(true)
        .with_search_start(common_args.workspace_config.workspace_locator_start())
        .with_ignore_pixi_version_check(true)
        .locate();
    match workspace {
        Err(WorkspaceLocatorError::WorkspaceNotFound(_)) => {
            if common_args.local {
                return Err(miette::miette!(
                    "--local flag can only be used inside a pixi workspace but no workspace could be found",
                ));
            }
            Ok(None)
        }
        Err(e) => {
            if common_args.local {
                return Err(e).into_diagnostic().context("--local flag can only be used inside a pixi workspace but loading the workspace failed",);
            }
            Ok(None)
        }
        Ok(project) => Ok(Some(project.root().to_path_buf())),
    }
}

/// Load the configuration the given arguments select, taking the global layer
/// from `source`.
fn load_config(common_args: &CommonArgs, source: &GlobalConfigSource) -> miette::Result<Config> {
    let ret = if common_args.system {
        Config::load_system()
    } else if common_args.global {
        Config::load_global_with(source)
    } else if let Some(root) = determine_project_root(common_args)? {
        Config::load_with(&root, source)
    } else {
        Config::load_global_with(source)
    };

    Ok(ret)
}

fn determine_config_write_path(common_args: &CommonArgs) -> miette::Result<PathBuf> {
    let write_path = if common_args.system {
        pixi_config::config_path_system()
    } else {
        if let Some(root) = determine_project_root(common_args)?
            && !common_args.global
        {
            return Ok(root.join(consts::PIXI_DIR).join(consts::CONFIG_FILE));
        }

        let mut global_locations = pixi_config::config_path_global();
        let mut to = global_locations
            .pop()
            .expect("should have at least one global config path");

        for p in global_locations {
            if p.exists() {
                to = p;
                break;
            }
        }

        to
    };

    Ok(write_path)
}

fn alter_config(
    common_args: &CommonArgs,
    key: &str,
    value: Option<String>,
    mode: AlterMode,
) -> miette::Result<()> {
    let to = determine_config_write_path(common_args)?;

    // Edit only the file that is about to be written. Starting from the
    // merged config would bake every inherited setting into it, so the user
    // would silently stop following the layers they inherit from.
    let mut config = match Config::from_path(&to) {
        Ok(config) => config,
        Err(ConfigError::FileNotFound(_)) => Config::default(),
        Err(e) => return Err(e).into_diagnostic(),
    };

    match mode {
        AlterMode::Prepend | AlterMode::Append => {
            let is_prepend = matches!(mode, AlterMode::Prepend);

            match key {
                // `default-channels` replaces the lower layers rather than
                // extending them, so the list written here has to be the
                // whole one the user sees.
                "default-channels" => {
                    let input = value.expect("value must be provided");
                    let channel = NamedChannelOrUrl::from_str(&input)
                        .into_diagnostic()
                        .context("invalid channel name")?;
                    let mut new_channels =
                        load_config(common_args, &GlobalConfigSource::Search)?.default_channels;
                    if is_prepend {
                        new_channels.insert(0, channel);
                    } else {
                        new_channels.push(channel);
                    }
                    config.default_channels = new_channels;
                }
                // `extra-index-urls` is concatenated across layers, so only
                // this file's own share of the list is edited; copying the
                // lower layers in would list them twice. A prepend therefore
                // lands ahead of this file's URLs, but after the lower ones.
                "pypi-config.extra-index-urls" => {
                    let input = url::Url::parse(&value.expect("value must be provided"))
                        .map_err(|e| miette::miette!("Invalid URL: {}", e))?;
                    let mut new_urls = config.pypi_config.extra_index_urls.clone();
                    if is_prepend {
                        new_urls.insert(0, input);
                    } else {
                        new_urls.push(input);
                    }
                    config.pypi_config.extra_index_urls = new_urls;
                }
                _ => {
                    let list_keys = ["default-channels", "pypi-config.extra-index-urls"];
                    let msg_cmd = if is_prepend { "prepend" } else { "append" };
                    return Err(miette::miette!(
                        "{} is only supported for list keys: {}",
                        msg_cmd,
                        list_keys.join(", ")
                    ));
                }
            }
        }
        AlterMode::Set | AlterMode::Unset => config.set(key, value)?,
    }

    config.save(&to)?;
    eprintln!("✅ Updated config at {}", to.display());
    Ok(())
}

// Trick to show only relevant field of the Config
fn partial_config(config: &mut Config, key: &str) -> miette::Result<()> {
    let mut new = Config::default();

    match key {
        // Top-level configuration fields
        "default-channels" => new.default_channels = config.default_channels.clone(),
        "shell" => new.shell = config.shell.clone(),
        "tls-no-verify" => new.tls_no_verify = config.tls_no_verify,
        "tls-root-certs" => new.tls_root_certs = config.tls_root_certs,
        "offline" => new.offline = config.offline,
        "authentication-override-file" => {
            new.authentication_override_file = config.authentication_override_file.clone()
        }
        "mirrors" => new.mirrors = config.mirrors.clone(),
        "repodata-config" => new.repodata_config = config.repodata_config.clone(),
        "index-config" => new.index_config = config.index_config.clone(),
        "pypi-config" => new.pypi_config = config.pypi_config.clone(),
        "proxy-config" => new.proxy_config = config.proxy_config.clone(),
        "allow-symbolic-links" => new.allow_symbolic_links = config.allow_symbolic_links,
        "allow-hard-links" => new.allow_hard_links = config.allow_hard_links,
        "allow-ref-links" => new.allow_ref_links = config.allow_ref_links,
        "detached-environments" => new.detached_environments = config.detached_environments.clone(),
        "pinning-strategy" => new.pinning_strategy = config.pinning_strategy,
        "concurrency" => new.concurrency = config.concurrency.clone(),
        "run-post-link-scripts" => new.run_post_link_scripts = config.run_post_link_scripts.clone(),
        "s3-options" => new.s3_options = config.s3_options.clone(),
        "tool-platform" => new.tool_platform = config.tool_platform,
        "cache" => new.cache = config.cache.clone(),
        "experimental" => new.experimental = config.experimental.clone(),
        "build" => new.build = config.build.clone(),

        // Subkeys: shell
        "shell.change-ps1" => new.shell.change_ps1 = config.shell.change_ps1,
        "shell.force-activate" => new.shell.force_activate = config.shell.force_activate,
        "shell.source-completion-scripts" => {
            new.shell.source_completion_scripts = config.shell.source_completion_scripts;
        }

        // Subkeys: cache
        "cache.root" => new.cache.root = config.cache.root.clone(),
        "cache.conda-packages" => new.cache.conda_packages = config.cache.conda_packages.clone(),
        "cache.repodata" => new.cache.repodata = config.cache.repodata.clone(),
        "cache.pypi-wheels" => new.cache.pypi_wheels = config.cache.pypi_wheels.clone(),
        "cache.pypi-mapping" => new.cache.pypi_mapping = config.cache.pypi_mapping.clone(),
        "cache.exec-environments" => {
            new.cache.exec_environments = config.cache.exec_environments.clone()
        }
        "cache.build-tool-environments" => {
            new.cache.build_tool_environments = config.cache.build_tool_environments.clone()
        }
        "cache.detached-environments" => {
            new.cache.detached_environments = config.cache.detached_environments.clone()
        }
        "cache.netfs-redirect" => new.cache.netfs_redirect = config.cache.netfs_redirect,

        // Subkeys: proxy-config
        "proxy-config.http" => new.proxy_config.http = config.proxy_config.http.clone(),
        "proxy-config.https" => new.proxy_config.https = config.proxy_config.https.clone(),
        "proxy-config.non-proxy-hosts" => {
            new.proxy_config.non_proxy_hosts = config.proxy_config.non_proxy_hosts.clone()
        }

        // Subkeys: pypi-config
        "pypi-config.index-url" => new.pypi_config.index_url = config.pypi_config.index_url.clone(),
        "pypi-config.extra-index-urls" => {
            new.pypi_config.extra_index_urls = config.pypi_config.extra_index_urls.clone()
        }
        "pypi-config.keyring-provider" => {
            new.pypi_config.keyring_provider = config.pypi_config.keyring_provider.clone()
        }
        "pypi-config.allow-insecure-host" => {
            new.pypi_config.allow_insecure_host = config.pypi_config.allow_insecure_host.clone()
        }

        // Subkeys: repodata-config
        "repodata-config.disable-bzip2" => {
            new.repodata_config.default.disable_bzip2 = config.repodata_config.default.disable_bzip2
        }
        "repodata-config.disable-sharded" => {
            new.repodata_config.default.disable_sharded =
                config.repodata_config.default.disable_sharded
        }
        "repodata-config.disable-zstd" => {
            new.repodata_config.default.disable_zstd = config.repodata_config.default.disable_zstd
        }

        // Subkeys: index-config
        "index-config.base-url" => {
            new.index_config.default.base_url = config.index_config.default.base_url.clone()
        }
        "index-config.write-shards" => {
            new.index_config.default.write_shards = config.index_config.default.write_shards
        }
        "index-config.write-zst" => {
            new.index_config.default.write_zst = config.index_config.default.write_zst
        }

        // Subkeys: experimental
        "experimental.conda-script" => {
            new.experimental.conda_script = config.experimental.conda_script
        }
        "experimental.use-environment-activation-cache" => {
            new.experimental.use_environment_activation_cache =
                config.experimental.use_environment_activation_cache
        }

        // Subkeys: concurrency
        "concurrency.downloads" => {
            new.concurrency = config.concurrency.clone();
        }
        "concurrency.solves" => {
            new.concurrency = config.concurrency.clone();
        }

        // Subkeys: s3-options
        key if key.starts_with("s3-options.") => {
            if let Some(subkey) = key.strip_prefix("s3-options.") {
                if let Some((bucket, rest)) = subkey.split_once('.') {
                    if !["endpoint-url", "region", "force-path-style"].contains(&rest) {
                        let keys = config.get_keys();
                        return Err(miette::miette!("key must be one of: {}", keys.join(", ")));
                    }
                    if let Some(opts) = config.s3_options.0.get(bucket) {
                        new.s3_options.0.insert(bucket.to_string(), opts.clone());
                    }
                } else if let Some(opts) = config.s3_options.0.get(subkey) {
                    new.s3_options.0.insert(subkey.to_string(), opts.clone());
                }
            }
        }

        _ => {
            let keys = config.get_keys();
            return Err(miette::miette!("key must be one of: {}", keys.join(", ")));
        }
    }

    *config = new;

    Ok(())
}

fn prune_subkeys(out: &mut String, key: &str, json: bool) -> miette::Result<()> {
    if let Some(field) = key.strip_prefix("concurrency.") {
        if json {
            if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(out)
                && let Some(concurrency) = json_val
                    .get_mut("concurrency")
                    .and_then(|v| v.as_object_mut())
            {
                concurrency.retain(|k, _| k == field);
                *out = serde_json::to_string_pretty(&json_val).into_diagnostic()?;
            }
        } else if let Ok(mut doc) = out.parse::<toml_edit::DocumentMut>()
            && let Some(table) = doc
                .get_mut("concurrency")
                .and_then(|i| i.as_table_like_mut())
        {
            let to_remove: Vec<String> = table
                .iter()
                .map(|(k, _)| k.to_string())
                .filter(|k| k != field)
                .collect();
            for k in to_remove {
                table.remove(&k);
            }
            *out = doc.to_string();
        }
    } else if let Some(rest) = key.strip_prefix("s3-options.")
        && let Some((bucket, field)) = rest.split_once('.')
    {
        if json {
            if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(out)
                && let Some(bucket_obj) = json_val
                    .get_mut("s3-options")
                    .and_then(|v| v.get_mut(bucket))
                    .and_then(|v| v.as_object_mut())
            {
                bucket_obj.retain(|k, _| k == field);
                *out = serde_json::to_string_pretty(&json_val).into_diagnostic()?;
            }
        } else if let Ok(mut doc) = out.parse::<toml_edit::DocumentMut>()
            && let Some(bucket_table) = doc
                .get_mut("s3-options")
                .and_then(|i| i.as_table_like_mut())
                .and_then(|t| t.get_mut(bucket))
                .and_then(|i| i.as_table_like_mut())
        {
            let to_remove: Vec<String> = bucket_table
                .iter()
                .map(|(k, _)| k.to_string())
                .filter(|k| k != field)
                .collect();
            for k in to_remove {
                bucket_table.remove(&k);
            }
            *out = doc.to_string();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_config::{
        CacheConfig, ConcurrencyConfig, Config, ExperimentalConfig, IndexChannelConfig,
        IndexConfig, ProxyConfig, PyPIConfig, RepodataChannelConfig, RepodataConfig, ShellConfig,
        TlsRootCerts,
    };
    use std::path::PathBuf;

    #[test]
    fn test_partial_config_tls_root_certs() {
        let mut config = Config {
            tls_root_certs: Some(TlsRootCerts::Webpki),
            ..Default::default()
        };
        partial_config(&mut config, "tls-root-certs").unwrap();
        assert_eq!(config.tls_root_certs, Some(TlsRootCerts::Webpki));
    }

    #[test]
    fn test_partial_config_concurrency() {
        let mut config = Config {
            concurrency: ConcurrencyConfig {
                solves: 10,
                downloads: 20,
            },
            ..Default::default()
        };
        partial_config(&mut config, "concurrency").unwrap();
        assert_eq!(config.concurrency.solves, 10);
        assert_eq!(config.concurrency.downloads, 20);
    }

    #[test]
    fn test_partial_config_concurrency_solves() {
        let mut config = Config {
            concurrency: ConcurrencyConfig {
                solves: 10,
                downloads: 20,
            },
            ..Default::default()
        };
        partial_config(&mut config, "concurrency.solves").unwrap();
        assert_eq!(config.concurrency.solves, 10);

        let mut toml = toml_edit::ser::to_string_pretty(&config).unwrap();
        prune_subkeys(&mut toml, "concurrency.solves", false).unwrap();
        assert!(toml.contains("solves = 10"));
        assert!(!toml.contains("downloads"));

        let mut json = serde_json::to_string_pretty(&config).unwrap();
        prune_subkeys(&mut json, "concurrency.solves", true).unwrap();
        assert!(json.contains("\"solves\": 10"));
        assert!(!json.contains("\"downloads\""));
    }

    #[test]
    fn test_partial_config_concurrency_downloads() {
        let mut config = Config {
            concurrency: ConcurrencyConfig {
                solves: 10,
                downloads: 20,
            },
            ..Default::default()
        };
        partial_config(&mut config, "concurrency.downloads").unwrap();
        assert_eq!(config.concurrency.downloads, 20);

        let mut toml = toml_edit::ser::to_string_pretty(&config).unwrap();
        prune_subkeys(&mut toml, "concurrency.downloads", false).unwrap();
        assert!(toml.contains("downloads = 20"));
        assert!(!toml.contains("solves"));

        let mut json = serde_json::to_string_pretty(&config).unwrap();
        prune_subkeys(&mut json, "concurrency.downloads", true).unwrap();
        assert!(json.contains("\"downloads\": 20"));
        assert!(!json.contains("\"solves\""));
    }

    #[test]
    fn test_partial_config_cache_subkeys() {
        let mut config = Config {
            cache: CacheConfig {
                conda_packages: Some(PathBuf::from("/custom/cache/conda")),
                build_tool_environments: Some(PathBuf::from("/custom/cache/build")),
                ..Default::default()
            },
            ..Default::default()
        };
        partial_config(&mut config, "cache.conda-packages").unwrap();
        assert_eq!(
            config.cache.conda_packages,
            Some(PathBuf::from("/custom/cache/conda"))
        );
        assert_eq!(config.cache.build_tool_environments, None);
    }

    #[test]
    fn test_partial_config_shell_subkeys() {
        let mut config = Config {
            shell: ShellConfig {
                change_ps1: Some(false),
                force_activate: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        partial_config(&mut config, "shell.change-ps1").unwrap();
        assert_eq!(config.shell.change_ps1, Some(false));
        assert_eq!(config.shell.force_activate, None);
    }

    #[test]
    fn test_partial_config_proxy_subkeys() {
        let mut config = Config {
            proxy_config: ProxyConfig {
                http: Some(url::Url::parse("http://proxy.example.com").unwrap()),
                https: Some(url::Url::parse("https://proxy.example.com").unwrap()),
                ..Default::default()
            },
            ..Default::default()
        };
        partial_config(&mut config, "proxy-config.http").unwrap();
        assert_eq!(
            config.proxy_config.http,
            Some(url::Url::parse("http://proxy.example.com").unwrap())
        );
        assert_eq!(config.proxy_config.https, None);
    }

    #[test]
    fn test_partial_config_pypi_subkeys() {
        let mut config = Config {
            pypi_config: PyPIConfig {
                index_url: Some(url::Url::parse("https://pypi.org/simple").unwrap()),
                ..Default::default()
            },
            ..Default::default()
        };
        partial_config(&mut config, "pypi-config.index-url").unwrap();
        assert_eq!(
            config.pypi_config.index_url,
            Some(url::Url::parse("https://pypi.org/simple").unwrap())
        );
        assert_eq!(config.pypi_config.extra_index_urls, Vec::<url::Url>::new());
    }

    #[test]
    fn test_partial_config_repodata_subkeys() {
        let mut config = Config {
            repodata_config: RepodataConfig {
                default: RepodataChannelConfig {
                    disable_bzip2: Some(true),
                    disable_zstd: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        partial_config(&mut config, "repodata-config.disable-bzip2").unwrap();
        assert_eq!(config.repodata_config.default.disable_bzip2, Some(true));
        assert_eq!(config.repodata_config.default.disable_zstd, None);
    }

    #[test]
    fn test_partial_config_index_subkeys() {
        let mut config = Config {
            index_config: IndexConfig {
                default: IndexChannelConfig {
                    base_url: Some("https://index.example.com".to_string()),
                    write_zst: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        partial_config(&mut config, "index-config.base-url").unwrap();
        assert_eq!(
            config.index_config.default.base_url,
            Some("https://index.example.com".to_string())
        );
        assert_eq!(config.index_config.default.write_zst, None);
    }

    #[test]
    fn test_partial_config_experimental_subkeys() {
        let mut config = Config {
            experimental: ExperimentalConfig {
                conda_script: Some(true),
                use_environment_activation_cache: Some(false),
            },
            ..Default::default()
        };
        partial_config(&mut config, "experimental.conda-script").unwrap();
        assert_eq!(config.experimental.conda_script, Some(true));
        assert_eq!(config.experimental.use_environment_activation_cache, None);
    }

    #[test]
    fn test_partial_config_s3_options() {
        let mut config = Config::default();
        let s3_opts = pixi_config::S3Options {
            endpoint_url: url::Url::parse("https://s3.example.com").unwrap(),
            region: "us-west-2".to_string(),
            force_path_style: true,
        };
        config.s3_options.0.insert("mybucket".to_string(), s3_opts);
        partial_config(&mut config, "s3-options").unwrap();
        assert!(config.s3_options.0.contains_key("mybucket"));

        let mut config2 = Config::default();
        config2.s3_options.0.insert(
            "mybucket".to_string(),
            pixi_config::S3Options {
                endpoint_url: url::Url::parse("https://s3.example.com").unwrap(),
                region: "us-west-2".to_string(),
                force_path_style: true,
            },
        );
        partial_config(&mut config2, "s3-options.mybucket").unwrap();
        assert!(config2.s3_options.0.contains_key("mybucket"));

        let mut config3 = Config::default();
        config3.s3_options.0.insert(
            "mybucket".to_string(),
            pixi_config::S3Options {
                endpoint_url: url::Url::parse("https://s3.example.com").unwrap(),
                region: "us-west-2".to_string(),
                force_path_style: true,
            },
        );
        partial_config(&mut config3, "s3-options.mybucket.region").unwrap();
        let mut toml = toml_edit::ser::to_string_pretty(&config3).unwrap();
        prune_subkeys(&mut toml, "s3-options.mybucket.region", false).unwrap();
        assert!(toml.contains("region = \"us-west-2\""));
        assert!(!toml.contains("endpoint-url"));
        assert!(!toml.contains("force-path-style"));
    }

    #[test]
    fn test_partial_config_invalid_key_error() {
        let mut config = Config::default();
        let err = partial_config(&mut config, "invalid-key").unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("key must be one of:"));
        assert!(err_str.contains("tls-root-certs"));
        assert!(err_str.contains("concurrency"));
        assert!(err_str.contains("build"));
    }

    #[test]
    fn test_partial_config_all_supported_keys() {
        let base_config = Config::default();
        let keys = base_config.get_keys();
        for key in keys {
            let mut config = Config::default();
            let concrete_key = key.replace("<bucket>", "testbucket");
            let result = partial_config(&mut config, &concrete_key);
            assert!(
                result.is_ok(),
                "Key '{concrete_key}' failed partial_config: {:?}",
                result.err()
            );
        }
    }
}
