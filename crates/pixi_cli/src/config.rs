use crate::cli_config::WorkspaceConfig;
use clap::Parser;
use miette::{IntoDiagnostic, WrapErr};
use pixi_config;
use pixi_config::{Config, ConfigError, GlobalConfigSource};
use pixi_consts::consts;
use pixi_core::WorkspaceLocator;
use pixi_core::workspace::WorkspaceLocatorError;
use rattler_conda_types::NamedChannelOrUrl;
use std::{
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

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
            pixi_utils::io::ignore_broken_pipe(writeln!(std::io::stdout(), "{out}"))
                .into_diagnostic()?;
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
        Subcommand::Set(args) => {
            let mode = if args.value.is_none() {
                AlterMode::Unset
            } else {
                AlterMode::Set
            };
            alter_config(&args.common, &args.key, args.value, mode)?;
        }
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

fn parse_key_path(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = '"';

    for ch in input.chars() {
        if in_quotes {
            if ch == quote_char {
                in_quotes = false;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quotes = true;
            quote_char = ch;
        } else if ch == '.' {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() || input.ends_with('.') {
        parts.push(current.trim().to_string());
    }
    parts
}

fn remove_table_key_recursive(table: &mut dyn toml_edit::TableLike, parts: &[String]) -> bool {
    if parts.is_empty() {
        return false;
    }
    if parts.len() == 1 {
        table.remove(&parts[0]).is_some()
    } else {
        let first = &parts[0];
        let rest = &parts[1..];
        if let Some(item) = table.get_mut(first)
            && let Some(subtable) = item.as_table_like_mut()
        {
            let removed = remove_table_key_recursive(subtable, rest);
            if subtable.is_empty() {
                table.remove(first);
            }
            return removed;
        }
        false
    }
}

fn unset_config_key(path: &Path, key: &str) -> miette::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let content = fs_err::read_to_string(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read config file '{}'", path.display()))?;

    let mut doc: toml_edit::DocumentMut = match content.parse() {
        Ok(doc) => doc,
        Err(_) => return Ok(()),
    };

    let parts = parse_key_path(key);
    if parts.is_empty() {
        return Ok(());
    }

    remove_table_key_recursive(doc.as_table_mut(), &parts);

    let parent = path.parent().expect("config path should have a parent");
    fs_err::create_dir_all(parent)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create directories in '{}'", parent.display()))?;

    fs_err::write(path, doc.to_string())
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to write config to '{}'", path.display()))?;

    Ok(())
}

fn alter_config(
    common_args: &CommonArgs,
    key: &str,
    value: Option<String>,
    mode: AlterMode,
) -> miette::Result<()> {
    let to = determine_config_write_path(common_args)?;

    if matches!(mode, AlterMode::Unset) {
        unset_config_key(&to, key)?;
        eprintln!("Updated config at {}", to.display());
        return Ok(());
    }

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
        AlterMode::Set => config.set(key, value)?,
        AlterMode::Unset => unreachable!(),
    }

    config.save(&to)?;
    eprintln!("Updated config at {}", to.display());
    Ok(())
}

// Trick to show only relevant field of the Config
fn partial_config(config: &mut Config, key: &str) -> miette::Result<()> {
    let mut new = Config::default();

    match key {
        "default-channels" => new.default_channels = config.default_channels.clone(),
        "shell" => new.shell = config.shell.clone(),
        "tls-no-verify" => new.tls_no_verify = config.tls_no_verify,
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
        _ => {
            let keys = [
                "default-channels",
                "tls-no-verify",
                "offline",
                "authentication-override-file",
                "mirrors",
                "repodata-config",
                "index-config",
                "pypi-config",
                "proxy-config",
                "allow-symbolic-links",
                "allow-hard-links",
                "allow-ref-links",
            ];
            return Err(miette::miette!("key must be one of: {}", keys.join(", ")));
        }
    }

    *config = new;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_key_path() {
        assert_eq!(parse_key_path("offline"), vec!["offline"]);
        assert_eq!(
            parse_key_path("shell.change-ps1"),
            vec!["shell", "change-ps1"]
        );
        assert_eq!(
            parse_key_path("repodata-config.disable-jlap"),
            vec!["repodata-config", "disable-jlap"]
        );
        assert_eq!(
            parse_key_path(r#"repodata-config."https://prefix.dev".disable-bzip2"#),
            vec!["repodata-config", "https://prefix.dev", "disable-bzip2"]
        );
    }

    #[test]
    fn test_unset_config_key_unknown_key() {
        let file = NamedTempFile::new().unwrap();
        let initial_toml = r#"
[repodata-config]
disable-jlap = true
disable-bzip2 = false
"#;
        fs_err::write(file.path(), initial_toml).unwrap();

        unset_config_key(file.path(), "repodata-config.disable-jlap").unwrap();

        let modified = fs_err::read_to_string(file.path()).unwrap();
        assert!(!modified.contains("disable-jlap"));
        assert!(modified.contains("disable-bzip2 = false"));
    }

    #[test]
    fn test_unset_config_key_removes_empty_parent_table() {
        let file = NamedTempFile::new().unwrap();
        let initial_toml = r#"
[repodata-config]
disable-jlap = true
"#;
        fs_err::write(file.path(), initial_toml).unwrap();

        unset_config_key(file.path(), "repodata-config.disable-jlap").unwrap();

        let modified = fs_err::read_to_string(file.path()).unwrap();
        assert!(!modified.contains("disable-jlap"));
        assert!(!modified.contains("[repodata-config]"));
    }

    #[test]
    fn test_unset_config_key_missing_key_does_not_error() {
        let file = NamedTempFile::new().unwrap();
        let initial_toml = r#"
default-channels = ["conda-forge"]
"#;
        fs_err::write(file.path(), initial_toml).unwrap();

        unset_config_key(file.path(), "unknown-key").unwrap();
        unset_config_key(file.path(), "repodata-config.disable-jlap").unwrap();

        let modified = fs_err::read_to_string(file.path()).unwrap();
        assert!(modified.contains(r#"default-channels = ["conda-forge"]"#));
    }

    #[test]
    fn test_unset_config_key_preserves_comments() {
        let file = NamedTempFile::new().unwrap();
        let initial_toml = r#"# Top-level comment
default-channels = ["conda-forge"]

# Shell options
[shell]
change-ps1 = false
force-activate = true
"#;
        fs_err::write(file.path(), initial_toml).unwrap();

        unset_config_key(file.path(), "shell.change-ps1").unwrap();

        let modified = fs_err::read_to_string(file.path()).unwrap();
        assert!(modified.contains("# Top-level comment"));
        assert!(modified.contains("# Shell options"));
        assert!(!modified.contains("change-ps1"));
        assert!(modified.contains("force-activate = true"));
    }
}
