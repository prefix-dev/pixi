use crate::cli_config::WorkspaceConfig;
use clap::Parser;
use miette::{IntoDiagnostic, WrapErr};
use pixi_config;
use pixi_config::Config;
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

    /// Describe every configuration option with its type, default, and a short explanation
    #[arg(long)]
    describe: bool,

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
            let mut config = load_config(&args.common)?;

            if args.describe {
                let out = render_describe(&config, args.key.as_deref(), args.json)?;
                writeln!(std::io::stdout(), "{out}")
                    .map_err(|e| {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            std::process::exit(0);
                        }
                        e
                    })
                    .into_diagnostic()?;
                return Ok(());
            }

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
            writeln!(std::io::stdout(), "{out}")
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        std::process::exit(0);
                    }
                    e
                })
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

fn render_describe(
    config: &Config,
    key_filter: Option<&str>,
    json: bool,
) -> miette::Result<String> {
    let descriptions = config.describe_keys();

    let selected: Vec<&pixi_config::ConfigOptionDescription> = if let Some(k) = key_filter {
        let matches: Vec<_> = descriptions.iter().filter(|o| o.key == k).collect();
        if matches.is_empty() {
            return Err(miette::miette!(
                "Unknown configuration key '{}'. Run `pixi config list --describe` to list every available key.",
                k
            ));
        }
        matches
    } else {
        descriptions.iter().collect()
    };

    let doc = toml_edit::ser::to_string_pretty(config)
        .into_diagnostic()
        .and_then(|s| s.parse::<toml_edit::DocumentMut>().into_diagnostic())?;

    if json {
        let arr: Vec<serde_json::Value> = selected
            .iter()
            .map(|opt| {
                let value = if opt.key.contains("<bucket>") {
                    serde_json::Value::Null
                } else {
                    lookup_dotted(doc.as_table(), opt.key)
                        .map(toml_item_to_json)
                        .unwrap_or(serde_json::Value::Null)
                };
                serde_json::json!({
                    "key": opt.key,
                    "description": opt.description,
                    "type": opt.value_type,
                    "default": opt.default,
                    "value": value,
                })
            })
            .collect();
        return serde_json::to_string_pretty(&arr).into_diagnostic();
    }

    let mut out = String::new();
    for (i, opt) in selected.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str("# ");
        out.push_str(opt.description);
        out.push('\n');
        out.push_str("# Type: ");
        out.push_str(opt.value_type);
        out.push('\n');
        out.push_str("# Default: ");
        out.push_str(opt.default);
        out.push('\n');

        let current = if opt.key.contains("<bucket>") {
            None
        } else {
            lookup_dotted(doc.as_table(), opt.key).map(format_toml_value)
        };

        match current {
            Some(value) => {
                out.push_str(opt.key);
                out.push_str(" = ");
                out.push_str(&value);
                out.push('\n');
            }
            None => {
                out.push_str("# ");
                out.push_str(opt.key);
                out.push_str(" = ");
                out.push_str(opt.default);
                out.push('\n');
            }
        }
    }
    Ok(out)
}

fn lookup_dotted<'a>(table: &'a toml_edit::Table, key: &str) -> Option<&'a toml_edit::Item> {
    let mut parts = key.split('.');
    let first = parts.next()?;
    let mut item = table.get(first)?;
    for part in parts {
        item = item.as_table().and_then(|t| t.get(part))?;
    }
    Some(item)
}

fn format_toml_value(item: &toml_edit::Item) -> String {
    match item {
        toml_edit::Item::Value(v) => v.to_string().trim().to_string(),
        toml_edit::Item::Table(t) => table_to_inline(t).to_string().trim().to_string(),
        toml_edit::Item::ArrayOfTables(arr) => {
            let mut out = toml_edit::Array::new();
            for t in arr {
                out.push(table_to_inline(t));
            }
            out.to_string().trim().to_string()
        }
        toml_edit::Item::None => String::new(),
    }
}

fn table_to_inline(table: &toml_edit::Table) -> toml_edit::InlineTable {
    let mut inline = toml_edit::InlineTable::new();
    for (k, v) in table.iter() {
        if let Some(val) = item_to_value(v) {
            inline.insert(k, val);
        }
    }
    inline
}

fn item_to_value(item: &toml_edit::Item) -> Option<toml_edit::Value> {
    match item {
        toml_edit::Item::Value(v) => Some(v.clone()),
        toml_edit::Item::Table(t) => Some(toml_edit::Value::InlineTable(table_to_inline(t))),
        toml_edit::Item::ArrayOfTables(arr) => {
            let mut out = toml_edit::Array::new();
            for t in arr {
                out.push(table_to_inline(t));
            }
            Some(toml_edit::Value::Array(out))
        }
        toml_edit::Item::None => None,
    }
}

fn toml_item_to_json(item: &toml_edit::Item) -> serde_json::Value {
    match item {
        toml_edit::Item::Value(v) => toml_value_to_json(v),
        toml_edit::Item::Table(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t.iter() {
                map.insert(k.to_string(), toml_item_to_json(v));
            }
            serde_json::Value::Object(map)
        }
        toml_edit::Item::ArrayOfTables(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|t| {
                    let mut map = serde_json::Map::new();
                    for (k, v) in t.iter() {
                        map.insert(k.to_string(), toml_item_to_json(v));
                    }
                    serde_json::Value::Object(map)
                })
                .collect(),
        ),
        toml_edit::Item::None => serde_json::Value::Null,
    }
}

fn toml_value_to_json(v: &toml_edit::Value) -> serde_json::Value {
    match v {
        toml_edit::Value::String(s) => serde_json::Value::String(s.value().to_string()),
        toml_edit::Value::Integer(i) => serde_json::Value::Number((*i.value()).into()),
        toml_edit::Value::Float(f) => serde_json::Number::from_f64(*f.value())
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml_edit::Value::Boolean(b) => serde_json::Value::Bool(*b.value()),
        toml_edit::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml_edit::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_value_to_json).collect())
        }
        toml_edit::Value::InlineTable(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t.iter() {
                map.insert(k.to_string(), toml_value_to_json(v));
            }
            serde_json::Value::Object(map)
        }
    }
}

// Trick to show only relevant field of the Config
fn partial_config(config: &mut Config, key: &str) -> miette::Result<()> {
    let mut new = Config::default();

    match key {
        "default-channels" => new.default_channels = config.default_channels.clone(),
        "shell" => new.shell = config.shell.clone(),
        "tls-no-verify" => new.tls_no_verify = config.tls_no_verify,
        "authentication-override-file" => {
            new.authentication_override_file = config.authentication_override_file.clone()
        }
        "mirrors" => new.mirrors = config.mirrors.clone(),
        "repodata-config" => new.repodata_config = config.repodata_config.clone(),
        "pypi-config" => new.pypi_config = config.pypi_config.clone(),
        "proxy-config" => new.proxy_config = config.proxy_config.clone(),
        "allow-symbolic-links" => new.allow_symbolic_links = config.allow_symbolic_links,
        "allow-hard-links" => new.allow_hard_links = config.allow_hard_links,
        "allow-ref-links" => new.allow_ref_links = config.allow_ref_links,
        _ => {
            let keys = [
                "default-channels",
                "tls-no-verify",
                "authentication-override-file",
                "mirrors",
                "repodata-config",
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
