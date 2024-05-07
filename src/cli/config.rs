use clap::Parser;
use miette::IntoDiagnostic;
use std::path::PathBuf;

use crate::{
    config::{home_path, Config},
    consts, project,
};

#[derive(Parser, Debug)]
enum Subcommand {
    /// Edit the configuration file
    Edit(EditArgs),

    /// List configuration values
    #[clap(visible_alias = "ls", alias = "l")]
    List(ListArgs),

    /// Set a configuration value
    Set(SetArgs),

    /// Unset a configuration value
    Unset(UnsetArgs),
}

#[derive(Parser, Debug, Clone)]
struct CommonArgs {
    /// operation on project-local configuration
    #[arg(long, conflicts_with_all = &["global", "system"])]
    local: bool,

    /// operation on global configuration
    #[arg(long, conflicts_with_all = &["local", "system"])]
    global: bool,

    /// operation on system configuration
    #[arg(long, conflicts_with_all = &["local", "global"])]
    system: bool,
}

#[derive(Parser, Debug, Clone)]
struct EditArgs {
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct ListArgs {
    /// output in JSON format
    #[arg(long)]
    json: bool,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct SetArgs {
    /// configuration key to set
    #[arg(required = true)]
    key: String,

    /// configuration value to set (key will be unset if value not provided)
    value: Option<String>,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct UnsetArgs {
    /// configuration key to unset
    #[arg(required = true)]
    key: String,

    #[clap(flatten)]
    common: CommonArgs,
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

            let config_path = determine_mutable_config_path(&args.common)?;
            let mut child = std::process::Command::new(editor.as_str())
                .arg(config_path)
                .spawn()
                .into_diagnostic()?;
            child.wait().into_diagnostic()?;
        }
        Subcommand::List(args) => {
            if args.common.system {
                panic!("Unimplemented");
            } else {
                let project_toml = project::find_project_manifest();

                let config = if let Some(project_toml) = project_toml {
                    let full_path = dunce::canonicalize(&project_toml).into_diagnostic()?;
                    let pixi_dir = full_path
                        .parent()
                        .ok_or_else(|| {
                            miette::miette!("can not find parent of {}", project_toml.display())
                        })?
                        .join(consts::PIXI_DIR);

                    if args.common.local {
                        let config_toml = pixi_dir.join(consts::CONFIG_FILE);
                        if let Ok(config) = Config::from_path(&config_toml) {
                            config
                        } else {
                            return Err(miette::miette!(
                                "there is no local config file for current project"
                            ));
                        }
                    } else {
                        Config::load(&pixi_dir)?
                    }
                } else if args.common.local {
                    return Err(miette::miette!(
                        "--local flag can only be used inside a pixi project"
                    ));
                } else {
                    Config::load_global()
                };

                let out = if args.json {
                    serde_json::to_string_pretty(&config).into_diagnostic()?
                } else {
                    toml_edit::ser::to_string_pretty(&config).into_diagnostic()?
                };

                eprintln!("{}", out);
            }
        }
        Subcommand::Set(args) => alter_config(&args.common, &args.key, args.value)?,
        Subcommand::Unset(args) => alter_config(&args.common, &args.key, None)?,
    };
    Ok(())
}

fn determine_mutable_config_path(common_args: &CommonArgs) -> miette::Result<PathBuf> {
    let mut config_path = None;

    if common_args.system {
        panic!("Unimplemented");
    } else if common_args.global {
        let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME").map_or_else(
            || dirs::home_dir().map(|d| d.join(".config")),
            |p| Some(PathBuf::from(p)),
        );

        let reserved_global_config_path = home_path().map(|d| d.join(consts::CONFIG_FILE));
        let global_locations = vec![
            xdg_config_home.map(|d| d.join("pixi").join(consts::CONFIG_FILE)),
            dirs::config_dir().map(|d| d.join("pixi").join(consts::CONFIG_FILE)),
        ];

        for location in global_locations.into_iter().flatten() {
            if location.exists() {
                config_path = Some(location);
                break;
            }
        }

        if config_path.is_none() {
            config_path = reserved_global_config_path;
        }
    } else {
        let project_toml = project::find_project_manifest();
        if common_args.local && project_toml.is_none() {
            return Err(miette::miette!(
                "--local flag can only be used inside a pixi project"
            ));
        }

        if let Some(project_toml) = project_toml {
            let full_path = dunce::canonicalize(&project_toml).into_diagnostic()?;
            let root = full_path.parent().ok_or_else(|| {
                miette::miette!("can not find parent of {}", project_toml.display())
            })?;
            config_path = Some(root.join(consts::PIXI_DIR).join(consts::CONFIG_FILE));
        } else {
            return Err(miette::miette!("not inside a pixi project"));
        }
    }

    config_path.ok_or_else(|| miette::miette!("can not determine config path"))
}

fn alter_config(common_args: &CommonArgs, key: &str, value: Option<String>) -> miette::Result<()> {
    let config_path = determine_mutable_config_path(common_args)?;
    let mut config = Config::from_path(&config_path)?;

    match key {
        "change_ps1" | "change-ps1" => config.set("change_ps1", value)?,
        "repodata_config.disable_jlap" | "repodata-config.disable-jlap" => {
            config.set("repodata_config.disable_jlap", value)?
        }
        "repodata_config.disable_bzip2" | "repodata-config.disable-bzip2" => {
            config.set("repodata_config.disable_bzip2", value)?
        }
        "repodata_config.disable_zstd" | "repodata-config.disable-zstd" => {
            config.set("repodata_config.disable_zstd", value)?
        }
        "pypi_config.index_url" | "pypi-config.index-url" => {
            config.set("pypi_config.index_url", value)?
        }
        "pypi_config.keyring_provider" | "pypi-config.keyring-provider" => {
            config.set("pypi_config.keyring_provider", value)?
        }
        _ => return Err(miette::miette!("unsopperted key {}", key)),
    }

    config.save()
}
