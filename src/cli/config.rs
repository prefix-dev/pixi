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
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct SetArgs {
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
struct UnsetArgs {
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

            let mut config_path = None;

            if args.common.system {
                panic!("Unimplemented");
            } else if args.common.global {
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
                if args.common.local && project_toml.is_none() {
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

            let mut child = std::process::Command::new(editor.as_str())
                .arg(config_path.expect("config path should be set"))
                .spawn()
                .into_diagnostic()?;
            child.wait().into_diagnostic()?;
        }
        Subcommand::List(args) => {
            if args.common.system {
                panic!("Unimplemented");
            } else {
                let project_toml = project::find_project_manifest();

                if let Some(project_toml) = project_toml {
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
                            eprint!("{:?}", config);
                        } else {
                            eprintln!("there is no local config file for current project");
                        }
                    } else {
                        let config = Config::load(&pixi_dir)?;
                        eprint!("{:?}", config);
                    }
                } else if args.common.local {
                    return Err(miette::miette!(
                        "--local flag can only be used inside a pixi project"
                    ));
                } else {
                    let config = Config::load_global();
                    eprint!("{:?}", config);
                }
            }
        }
        Subcommand::Set(_) => panic!("Unimplemented"),
        Subcommand::Unset(_) => panic!("Unimplemented"),
    };
    Ok(())
}
