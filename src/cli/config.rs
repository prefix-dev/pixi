use std::{path::PathBuf, str::FromStr};

use clap::Parser;
use miette::{IntoDiagnostic, WrapErr};
use pixi_config;
use pixi_config::Config;
use pixi_consts::consts;
use rattler_conda_types::NamedChannelOrUrl;

use crate::project;

#[derive(Parser, Debug)]
enum Subcommand {
    /// Edit the configuration file
    #[clap(alias = "e")]
    Edit(EditArgs),

    /// List configuration values
    ///
    /// Example:
    ///     pixi config list default-channels
    #[clap(visible_alias = "ls", alias = "l")]
    List(ListArgs),

    /// Prepend a value to a list configuration key
    ///
    /// Example:
    ///     pixi config prepend default-channels bioconda
    Prepend(PendArgs),

    /// Append a value to a list configuration key
    ///
    /// Example:
    ///     pixi config append default-channels bioconda
    Append(PendArgs),

    /// Set a configuration value
    ///
    /// Example:
    ///     pixi config set default-channels '["conda-forge", "bioconda"]'
    Set(SetArgs),

    /// Unset a configuration value
    ///
    /// Example:
    ///     pixi config unset default-channels
    Unset(UnsetArgs),
}

#[derive(Parser, Debug, Clone)]
struct CommonArgs {
    /// Operation on project-local configuration
    #[arg(long, short, conflicts_with_all = &["global", "system"])]
    local: bool,

    /// Operation on global configuration
    #[arg(long, short, conflicts_with_all = &["local", "system"])]
    global: bool,

    /// Operation on system configuration
    #[arg(long, short, conflicts_with_all = &["local", "global"])]
    system: bool,
}

#[derive(Parser, Debug, Clone)]
struct EditArgs {
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct ListArgs {
    /// Configuration key to show (all if not provided)
    key: Option<String>,

    /// Output in JSON format
    #[arg(long)]
    json: bool,

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
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
                #[cfg(not(target_os = "windows"))]
                {
                    "nano".to_string()
                }
                #[cfg(target_os = "windows")]
                {
                    "notepad".to_string()
                }
            });

            let config_path = determine_config_write_path(&args.common)?;
            let mut child = std::process::Command::new(editor.as_str())
                .arg(config_path)
                .spawn()
                .into_diagnostic()?;
            child.wait().into_diagnostic()?;
        }
        Subcommand::List(args) => {
            let mut config = load_config(&args.common)?;

            if let Some(key) = args.key {
                partial_config(&mut config, &key)?;
            }

            let out = if args.json {
                serde_json::to_string_pretty(&config).into_diagnostic()?
            } else {
                toml_edit::ser::to_string_pretty(&config).into_diagnostic()?
            };

            if out.is_empty() {
                eprintln!("Configuration not set");
            }
            println!("{}", out);
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
    match project::find_project_manifest() {
        None => {
            if common_args.local {
                return Err(miette::miette!(
                    "--local flag can only be used inside a pixi project"
                ));
            }
            Ok(None)
        }
        Some(manifest_file) => {
            let full_path = dunce::canonicalize(&manifest_file).into_diagnostic()?;
            let root = full_path
                .parent()
                .ok_or_else(|| {
                    miette::miette!("can not find parent of {}", manifest_file.display())
                })?
                .to_path_buf();
            Ok(Some(root))
        }
    }
}

fn load_config(common_args: &CommonArgs) -> miette::Result<Config> {
    let ret = if common_args.system {
        Config::load_system()
    } else if common_args.global {
        Config::load_global()
    } else if let Some(root) = determine_project_root(common_args)? {
        Config::load(&root)
    } else {
        Config::load_global()
    };

    Ok(ret)
}

fn determine_config_write_path(common_args: &CommonArgs) -> miette::Result<PathBuf> {
    let write_path = if common_args.system {
        pixi_config::config_path_system()
    } else {
        if let Some(root) = determine_project_root(common_args)? {
            if !common_args.global {
                return Ok(root.join(consts::PIXI_DIR).join(consts::CONFIG_FILE));
            }
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
    let mut config = load_config(common_args)?;
    let to = determine_config_write_path(common_args)?;

    match mode {
        AlterMode::Prepend | AlterMode::Append => {
            let is_prepend = matches!(mode, AlterMode::Prepend);

            match key {
                "default-channels" => {
                    let input = value.expect("value must be provided");
                    let channel = NamedChannelOrUrl::from_str(&input)
                        .into_diagnostic()
                        .context("invalid channel name")?;
                    let mut new_channels = config.default_channels.clone();
                    if is_prepend {
                        new_channels.insert(0, channel);
                    } else {
                        new_channels.push(channel);
                    }
                    config.default_channels = new_channels;
                }
                "pypi-config.extra-index-urls" => {
                    let input = url::Url::parse(&value.expect("value must be provided"))
                        .map_err(|e| miette::miette!("Invalid URL: {}", e))?;
                    let mut new_urls = config.pypi_config().extra_index_urls.clone();
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
        "default-channels" => new.default_channels = config.default_channels.clone(),
        "change-ps1" => new.change_ps1 = config.change_ps1,
        "tls-no-verify" => new.tls_no_verify = config.tls_no_verify,
        "authentication-override-file" => {
            new.authentication_override_file = config.authentication_override_file.clone()
        }
        "mirrors" => new.mirrors = config.mirrors.clone(),
        "repodata-config" => new.repodata_config = config.repodata_config.clone(),
        "pypi-config" => new.pypi_config = config.pypi_config.clone(),
        _ => {
            let keys = [
                "default-channels",
                "change-ps1",
                "tls-no-verify",
                "authentication-override-file",
                "mirrors",
                "repodata-config",
                "pypi-config",
            ];
            return Err(miette::miette!("key must be one of: {}", keys.join(", ")));
        }
    }

    *config = new;

    Ok(())
}
