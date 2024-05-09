use clap::Parser;
use miette::IntoDiagnostic;
use std::path::PathBuf;

use crate::{
    config::{self, Config},
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
    key: String,

    /// configuration value to set (key will be unset if value not provided)
    value: Option<String>,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct UnsetArgs {
    /// configuration key to unset
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

            let config_path = determine_config_write_path(&args.common)?;
            let mut child = std::process::Command::new(editor.as_str())
                .arg(config_path)
                .spawn()
                .into_diagnostic()?;
            child.wait().into_diagnostic()?;
        }
        Subcommand::List(args) => {
            let config = load_config(&args.common)?;
            let out = if args.json {
                serde_json::to_string_pretty(&config).into_diagnostic()?
            } else {
                toml_edit::ser::to_string_pretty(&config).into_diagnostic()?
            };

            if out.is_empty() {
                eprintln!("No configuration values found");
            } else {
                eprintln!("{}", out);
            }
        }
        Subcommand::Set(args) => alter_config(&args.common, &args.key, args.value)?,
        Subcommand::Unset(args) => alter_config(&args.common, &args.key, None)?,
    };
    Ok(())
}

fn determine_project_root(common_args: &CommonArgs) -> miette::Result<PathBuf> {
    let project_toml = project::find_project_manifest();
    if common_args.local && project_toml.is_none() {
        return Err(miette::miette!(
            "--local flag can only be used inside a pixi project"
        ));
    }

    if let Some(project_toml) = project_toml {
        let full_path = dunce::canonicalize(&project_toml).into_diagnostic()?;
        let root = full_path
            .parent()
            .ok_or_else(|| miette::miette!("can not find parent of {}", project_toml.display()))?
            .to_path_buf();
        Ok(root)
    } else {
        Err(miette::miette!("not inside a pixi project"))
    }
}

fn load_config(common_args: &CommonArgs) -> miette::Result<Config> {
    let ret = if common_args.system {
        Config::load_system()
    } else if common_args.global {
        Config::load_global()
    } else {
        let root = determine_project_root(common_args)?;
        Config::load(&root)
    };

    Ok(ret)
}

fn determine_config_write_path(common_args: &CommonArgs) -> miette::Result<PathBuf> {
    let write_path = if common_args.system {
        config::config_path_system()
    } else if common_args.global {
        let mut global_locations = config::config_path_global();
        let mut ret = global_locations
            .pop()
            .expect("should have at least one global config path");

        for p in global_locations {
            if p.exists() {
                ret = p;
                break;
            }
        }

        ret
    } else {
        determine_project_root(common_args)?
            .join(consts::PIXI_DIR)
            .join(consts::CONFIG_FILE)
    };

    Ok(write_path)
}

fn alter_config(common_args: &CommonArgs, key: &str, value: Option<String>) -> miette::Result<()> {
    let mut config = load_config(common_args)?;
    let to = determine_config_write_path(common_args)?;

    config.set(key, value)?;
    config.save(&to)
}
