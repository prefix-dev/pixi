use crate::cli_config::WorkspaceConfig;
use clap::Parser;
use miette::{IntoDiagnostic, WrapErr};
use pixi_config;
use pixi_config::{Config, ConfigError, GlobalConfigSource};
use pixi_consts::consts;
use pixi_core::workspace::WorkspaceLocatorError;
use pixi_core::{WorkspaceLocator, host::HostDetection};
use pixi_manifest::toml::TomlDocument;
use pixi_toml_edit::{insert_array_element, push_array_element, remove_entry, upsert_entry};
use rattler_conda_types::{ChannelUrl, NamedChannelOrUrl};
use serde_json::Value as JsonValue;
use std::{
    io::Write,
    iter::once,
    path::{Path, PathBuf},
    str::FromStr,
};
use toml_edit::{DocumentMut, Item, Key};

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
    #[arg(long, short, conflicts_with_all = &["global", "system", "shared", "path"], help_heading = consts::CLAP_CONFIG_OPTIONS)]
    local: bool,

    /// Operation on global configuration
    #[arg(long, short, conflicts_with_all = &["local", "system", "shared", "path"], help_heading = consts::CLAP_CONFIG_OPTIONS)]
    global: bool,

    /// Operation on system configuration
    #[arg(long, short, conflicts_with_all = &["local", "global", "shared", "path"], help_heading = consts::CLAP_CONFIG_OPTIONS)]
    system: bool,

    /// Operation on the configuration shared with other rattler-based tools
    /// (`~/.config/rattler/config.toml`), which only accepts the keys every
    /// such tool understands
    #[arg(long, conflicts_with_all = &["local", "global", "system", "path"], help_heading = consts::CLAP_CONFIG_OPTIONS)]
    shared: bool,

    /// Path to a local configuration file
    #[arg(long, short, conflicts_with_all = &["local", "global", "system", "shared"], help_heading = consts::CLAP_CONFIG_OPTIONS)]
    path: Option<PathBuf>,

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

#[derive(Debug)]
pub struct KeyPath {
    parent_keys: Vec<String>,
    target_key: String,
}

impl KeyPath {
    pub fn parse(key: &str) -> miette::Result<Self> {
        let mut parts = Key::parse(key)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to parse the key '{key}'"))?;

        if parts.is_empty() {
            return Err(miette::miette!("Key path cannot be empty"));
        }

        let target_key = parts
            .pop()
            .ok_or_else(|| miette::miette!("Expected a target key"))?
            .get()
            .to_string();
        let parent_keys = parts.into_iter().map(|k| k.get().to_string()).collect();

        Ok(Self {
            parent_keys,
            target_key,
        })
    }

    pub fn parents(&self) -> Vec<&str> {
        self.parent_keys.iter().map(|s| s.as_str()).collect()
    }

    pub fn target(&self) -> &str {
        &self.target_key
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.subcommand {
        Subcommand::Edit(args) => {
            let config_path = determine_config_write_path(&args.common).await?;

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
            let config = load_config(&args.common, &args.config_source.source()).await?;

            let out = if let Some(key) = args.key {
                let partial = partial_config(&config, &key)?;
                if args.json {
                    serde_json::to_string_pretty(&partial).into_diagnostic()?
                } else {
                    toml_edit::ser::to_string_pretty(&partial).into_diagnostic()?
                }
            } else if args.json {
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
        Subcommand::Prepend(args) => {
            alter_config(
                &args.common,
                &args.key,
                Some(args.value),
                AlterMode::Prepend,
            )
            .await?
        }
        Subcommand::Append(args) => {
            alter_config(&args.common, &args.key, Some(args.value), AlterMode::Append).await?
        }
        Subcommand::Set(args) => {
            alter_config(&args.common, &args.key, args.value, AlterMode::Set).await?
        }
        Subcommand::Unset(args) => {
            alter_config(&args.common, &args.key, None, AlterMode::Unset).await?
        }
    };
    Ok(())
}

async fn determine_project_root(common_args: &CommonArgs) -> miette::Result<Option<PathBuf>> {
    let workspace = WorkspaceLocator::default()
        .with_host(HostDetection::builtin())
        .with_closest_package(false) // Dont care about the package
        .with_emit_warnings(false) // No reason to emit warnings
        .with_consider_environment(true)
        .with_search_start(common_args.workspace_config.workspace_locator_start())
        .with_ignore_pixi_version_check(true)
        .locate()
        .await;
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
async fn load_config(
    common_args: &CommonArgs,
    source: &GlobalConfigSource,
) -> miette::Result<Config> {
    if common_args.system {
        return Ok(Config::load_system());
    }

    if common_args.shared {
        return Ok(Config::load_shared());
    }

    if common_args.global {
        return Ok(Config::load_global_with(source));
    }

    // If an explicit --path was given, load and merge that specific config file.
    if let Some(path) = &common_args.path {
        let base_config = Config::load_global_with(source);
        if global_source_contains_path(source, path) {
            return Ok(base_config);
        }

        let local_config = match Config::from_path(path) {
            Ok(config) => config,
            Err(ConfigError::FileNotFound(_)) => Config::default(),
            Err(error) => return Err(error).into_diagnostic(),
        };
        return Ok(base_config.merge_config(local_config));
    }

    // Otherwise, check if we are in a project/workspace root
    if let Some(root) = determine_project_root(common_args).await? {
        return Ok(Config::load_with(&root, source));
    }

    Ok(Config::load_global_with(source))
}

fn global_source_contains_path(source: &GlobalConfigSource, path: &Path) -> bool {
    match source {
        GlobalConfigSource::Search => pixi_config::config_search_locations()
            .iter()
            .any(|location| same_config_path(&location.path, path)),
        GlobalConfigSource::File(source_path) => same_config_path(source_path, path),
        GlobalConfigSource::None => false,
    }
}

fn same_config_path(left: &Path, right: &Path) -> bool {
    left == right
        || fs_err::canonicalize(left)
            .ok()
            .zip(fs_err::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

fn resolved_config_write_path(path: &Path) -> Option<PathBuf> {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(mut resolved) = fs_err::canonicalize(existing) {
            for component in missing.iter().rev() {
                resolved.push(component);
            }
            return Some(resolved);
        }
        missing.push(existing.file_name()?);
        existing = existing.parent()?;
        if existing.as_os_str().is_empty() {
            existing = Path::new(".");
        }
    }
}

async fn determine_config_write_path(common_args: &CommonArgs) -> miette::Result<PathBuf> {
    Ok(determine_config_write_destination(common_args).await?.0)
}

async fn determine_config_write_destination(
    common_args: &CommonArgs,
) -> miette::Result<(PathBuf, Option<PathBuf>)> {
    if let Some(path) = &common_args.path {
        let repository = determine_project_root(common_args).await?.filter(|root| {
            let local = root.join(consts::PIXI_DIR).join(consts::CONFIG_FILE);
            same_config_path(path, &local)
                || resolved_config_write_path(path)
                    .zip(resolved_config_write_path(&local))
                    .is_some_and(|(path, local)| path == local)
        });
        return Ok((path.clone(), repository));
    }

    if common_args.system {
        return Ok((pixi_config::config_path_system(), None));
    }

    if common_args.shared {
        return Ok((pixi_config::shared_user_config_write_path(), None));
    }

    if !common_args.global
        && let Some(root) = determine_project_root(common_args).await?
    {
        return Ok((
            root.join(consts::PIXI_DIR).join(consts::CONFIG_FILE),
            Some(root),
        ));
    }

    Ok((pixi_config::user_config_write_path(), None))
}

/// Alters a specific key in the user configuration file according to the given `mode`.
///
/// Handles reading the existing TOML document (or initializing a new one),
/// updating key values (including list modifications like `Prepend` and `Append`),
/// and persisting the formatted result back to the disk.
///
/// # Errors
///
/// - The target config path cannot be determined or created.
/// - The existing config file cannot be read or parsed as valid TOML.
/// - A list-only operation (`Prepend`/`Append`) is attempted on a non-list key.
/// - Persisting the updated content disk fails.
async fn alter_config(
    common_args: &CommonArgs,
    key: &str,
    value: Option<String>,
    mode: AlterMode,
) -> miette::Result<()> {
    let (mut to, repository) = determine_config_write_destination(common_args).await?;
    let content = match fs_err::read_to_string(&to) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e)
                .into_diagnostic()
                .context("failed to read config file");
        }
    };
    let keys_before = if common_args.shared {
        Config::keys_not_shared(&content)?
    } else {
        Vec::new()
    };

    let doc_mut = content
        .parse::<toml_edit::DocumentMut>()
        .into_diagnostic()
        .context("failed to parse TOML")?;

    let mut toml_doc = TomlDocument::new(doc_mut);

    // Edit only the file that is about to be written. Starting from the
    // merged config would bake every inherited setting into it, so the user
    // would silently stop following the layers they inherit from.
    let mut config = match Config::from_path(&to) {
        Ok(config) => config,
        Err(ConfigError::FileNotFound(_)) => Config::default(),
        Err(e) => return Err(e).into_diagnostic(),
    };

    let repository_consent = if let Some(root) = &repository {
        let segments = Key::parse(key).into_diagnostic()?;
        let is_consent = segments
            .first()
            .is_some_and(|segment| segment.get() == "virtual-package-detectors")
            && (segments.len() == 1
                || segments
                    .get(1)
                    .is_some_and(|segment| segment.get() == "consent"));
        if is_consent {
            to = pixi_config::repository_detector_config_path(root)?;
        }
        is_consent
    } else {
        false
    };
    match mode {
        AlterMode::Prepend | AlterMode::Append => {
            let is_prepend = matches!(mode, AlterMode::Prepend);
            let input = value.expect("value must be provided");

            match key {
                // `default-channels` replaces the lower layers rather than concatenating them.
                // If the local file has no override yet, we must write the entire merged list
                // so lower layers aren't accidentally silenced. If a local list already exists,
                // we append/prepend to it directly.
                "default-channels" => {
                    let channel = NamedChannelOrUrl::from_str(&input)
                        .into_diagnostic()
                        .context("invalid channel name")?;

                    let local_has_channels = toml_doc
                        .as_item()
                        .get("default-channels")
                        .or_else(|| toml_doc.as_item().get("default_channels"))
                        .is_some();

                    if local_has_channels {
                        // Local file already has default-channels. Modify the local list and use mode (Prepend/Append)
                        // to preserve existing multi-line TOML formatting.
                        if is_prepend {
                            config.default_channels.insert(0, channel);
                        } else {
                            config.default_channels.push(channel)
                        }
                        transplant_config_key(&config, &mut toml_doc, key, &mode)?;
                    } else {
                        // Local file is missing default-channels. Load inherited global layers,
                        // add the channel, and user AlterMode::Set to write the full merged array.
                        let mut new_channels =
                            load_config(common_args, &GlobalConfigSource::Search)
                                .await?
                                .default_channels;
                        if is_prepend {
                            new_channels.insert(0, channel);
                        } else {
                            new_channels.push(channel)
                        }
                        config.default_channels = new_channels;
                        transplant_config_key(&config, &mut toml_doc, key, &AlterMode::Set)?;
                    }
                }
                // `extra-index-urls` is concatenated across layers, so only
                // this file's own share of the list is edited; copying the
                // lower layers in would list them twice. A prepend therefore
                // lands ahead of this file's URLs, but after the lower ones.
                "pypi-config.extra-index-urls" => {
                    let url = url::Url::parse(&input)
                        .map_err(|e| miette::miette!("Invalid URL: {}", e))?;
                    if is_prepend {
                        config.pypi_config.extra_index_urls.insert(0, url);
                    } else {
                        config.pypi_config.extra_index_urls.push(url);
                    }
                    transplant_config_key(&config, &mut toml_doc, key, &mode)?;
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
        AlterMode::Set => {
            // Run set on Config object for validation
            config.set(key, value)?;

            transplant_config_key(&config, &mut toml_doc, key, &mode)?;
        }
        AlterMode::Unset => unset(&mut toml_doc, key)?,
    }

    let contents = toml_doc.to_string();
    if common_args.shared {
        // Only keys this edit adds are checked; a foreign key that was
        // already in the file is not this command's doing.
        let mut not_shared = Config::keys_not_shared(&contents)?;
        not_shared.retain(|key| !keys_before.contains(key));
        if !not_shared.is_empty() {
            return Err(miette::miette!(
                "'{}' is not a key shared by all rattler-based tools, so it cannot be written to \
                 the shared configuration. Use `--global` to set it for pixi alone.",
                not_shared.join("', '")
            ));
        }
    }
    let parent = to.parent().expect("config path should have a parent");
    fs_err::create_dir_all(parent)
        .into_diagnostic()
        .wrap_err(format!(
            "failed to create directories in '{}'",
            parent.display()
        ))?;
    if repository_consent && let Some(root) = &repository {
        to = pixi_config::repository_detector_config_path(root)?;
    }
    fs_err::write(&to, contents)
        .into_diagnostic()
        .wrap_err(format!("failed to write config to '{}'", to.display()))?;

    eprintln!("✅ Updated config at {}", to.display());
    Ok(())
}

/// Unset a key from the TOML document, preserving existing formatting and comments.
///
/// # Errors
///
/// - `key` is not a valid key path.
/// - The specified key does not exist in the document.
/// - The unset operation leaves the config as invalid.
fn unset(toml_doc: &mut TomlDocument, key: &str) -> miette::Result<()> {
    let key_path = KeyPath::parse(key)?;

    let top_level_table = key_path.parents().is_empty();

    let parent_table = if top_level_table {
        toml_doc.as_item_mut()
    } else {
        let parents_keys = resolve_parent_keys(toml_doc, &key_path.parents());
        let parents_strs: Vec<&str> = parents_keys.iter().map(|s| s.as_str()).collect();
        toml_doc
            .get_or_insert_nested_item(&parents_strs)
            .into_diagnostic()?
    };

    key_spellings(key_path.target())
        .iter()
        .find_map(|spelling| remove_entry(parent_table, spelling).ok().flatten())
        .ok_or_else(|| miette::miette!("Key '{}' not found in configuration file", key))?;

    prune_empty_parents(toml_doc, key_path.parents())?;

    Config::from_toml(&toml_doc.to_string(), None)
        .wrap_err_with(|| format!("Unsetting the {key} would leave the config file invalid"))?;

    Ok(())
}

fn prune_empty_parents(toml_doc: &mut TomlDocument, mut path: Vec<&str>) -> miette::Result<()> {
    if path.is_empty() {
        return Ok(());
    }

    let is_empty = toml_doc
        .get_nested_table(&path)
        .map(|t| t.is_empty())
        .unwrap_or(false);

    if is_empty {
        let Some(key_target_to_remove) = path.pop() else {
            return Ok(());
        };

        let parent_of_target = if path.is_empty() {
            toml_doc.as_item_mut()
        } else {
            toml_doc
                .get_or_insert_nested_item(&path)
                .into_diagnostic()?
        };

        remove_entry(parent_of_target, key_target_to_remove).into_diagnostic()?;

        prune_empty_parents(toml_doc, path)?;
    }

    Ok(())
}

/// Transplants a single key from a validated `Config` into an editable TOML document.
/// Empty values omitted by serialization remove the corresponding entry.
///
/// We serialize the entire Config and parse it into a temporary document because:
/// 1. The input value undergoes strict type validation via Serde.
/// 2. We extract only the specific target leaf node, preventing unrequested default values.
///
/// # Errors
///
/// - `key` is not a valid key path.
/// - Serializing `config` or parsing the temporary TOML document fails.
/// - Navigating or creating nested parent tables in `toml_doc` fails.
fn transplant_config_key(
    config: &Config,
    toml_doc: &mut TomlDocument,
    key: &str,
    mode: &AlterMode,
) -> miette::Result<()> {
    let key_path = KeyPath::parse(key)?;

    let full_serialized = toml_edit::ser::to_string(&config).into_diagnostic()?;
    let temp_doc = full_serialized.parse::<DocumentMut>().into_diagnostic()?;

    // Walk down all the way to the leaf. The serialized document spells every
    // segment the way the configuration does, which may differ from the key
    // as the user typed it.
    let mut current_item = temp_doc.as_item();
    for parent in key_path.parents() {
        current_item = get_any_spelling(current_item, parent).unwrap_or(&Item::None);
    }
    current_item = get_any_spelling(current_item, key_path.target()).unwrap_or(&Item::None);

    if current_item.is_none() {
        // fall back into unset
        match unset(toml_doc, key) {
            Ok(()) => return Ok(()),
            Err(e) if e.to_string().contains("not found in configuration file") => {
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }

    let target_table = if key_path.parents().is_empty() {
        Ok(toml_doc.as_item_mut())
    } else {
        let parents_keys = resolve_parent_keys(toml_doc, &key_path.parents());
        let parents_strs: Vec<&str> = parents_keys.iter().map(|s| s.as_str()).collect();
        toml_doc
            .get_or_insert_nested_item(&parents_strs)
            .into_diagnostic()
    }?;

    if let AlterMode::Append | AlterMode::Prepend = mode
        && let Some(new_value) = current_item.as_value()
        && let Some(serialized_array) = new_value.as_array()
    {
        // Check which key name to modify
        let array_key = legacy_alias(&key_path.target_key)
            .filter(|alias| {
                target_table
                    .as_table_like()
                    .is_some_and(|t| t.contains_key(alias))
            })
            .unwrap_or_else(|| key_path.target().to_string());

        let target_array = toml_doc
            .get_or_insert_toml_array_mut(&key_path.parents(), &array_key)
            .into_diagnostic()?;

        if matches!(mode, AlterMode::Prepend) {
            if let Some(new_item) = serialized_array.get(0) {
                insert_array_element(target_array, 0, new_item.clone());
            }
        } else if let Some(new_item) = serialized_array.iter().last() {
            push_array_element(target_array, new_item.clone());
        }
        return Ok(());
    }

    // Replace legacy snake_case keys while preserving their comments.
    if let Some(alias) = legacy_alias(&key_path.target_key) {
        let _ = remove_entry(target_table, &alias).into_diagnostic()?;
    }
    let target_key = key_spellings(key_path.target())
        .into_iter()
        .find(|spelling| target_table.get(spelling).is_some())
        .or_else(|| canonical_channel_url(key_path.target()))
        .unwrap_or_else(|| key_path.target().to_string());

    if let Some(value) = current_item.as_value() {
        upsert_entry(target_table, &target_key, value.clone()).into_diagnostic()?;
    } else if let Some(table_to_insert) = current_item.as_table()
        && let Some(table_like) = target_table.as_table_like_mut()
    {
        table_like.insert(&target_key, Item::Table(table_to_insert.clone()));
    }

    Ok(())
}

/// Looks `segment` up in `item` under any of its spellings.
fn get_any_spelling<'a>(item: &'a Item, segment: &str) -> Option<&'a Item> {
    key_spellings(segment)
        .iter()
        .find_map(|spelling| item.get(spelling))
}

/// The spellings a key segment may have in a configuration file: as given,
/// its legacy snake_case alias, and for a channel URL its canonical form,
/// which is how the configuration serializes it.
fn key_spellings(segment: &str) -> Vec<String> {
    let mut spellings = vec![segment.to_string()];
    spellings.extend(legacy_alias(segment));
    spellings.extend(canonical_channel_url(segment));
    if let Ok(url) = url::Url::parse(segment)
        && !url.cannot_be_a_base()
    {
        let channel = ChannelUrl::from(url);
        spellings.push(format!("{}/", channel.as_str().trim_end_matches('/')));
    }
    spellings
}

/// The spelling the configuration serializes a channel URL segment with,
/// when it differs from `segment`.
fn canonical_channel_url(segment: &str) -> Option<String> {
    let url = url::Url::parse(segment)
        .ok()
        .filter(|url| !url.cannot_be_a_base())?;
    let canonical = rattler_conda_types::ChannelUrl::from(url)
        .as_str()
        .trim_end_matches('/')
        .to_string();
    (canonical != segment).then_some(canonical)
}

/// Returns the legacy `snake_case` alias for a canonical `kebab-case` key or parent table
/// if the one exists in the Serde schema
fn legacy_alias(key: &str) -> Option<String> {
    match key {
        "default-channels"
        | "authentication-override-file"
        | "tls-no-verify"
        | "repodata-config"
        | "change-ps1"
        | "disable-bzip2"
        | "disable-sharded"
        | "disable-zstd" => Some(key.replace('-', "_")),
        _ => None,
    }
}

/// Resolve parent key paths against the TOML document, using existing snake_case aliases on disk if present.
fn resolve_parent_keys(doc: &TomlDocument, parents: &[&str]) -> Vec<String> {
    let mut resolved = Vec::with_capacity(parents.len());
    let mut current_item = doc.as_item();

    for &parent in parents {
        let spellings = key_spellings(parent);
        let present = spellings
            .iter()
            .find(|spelling| current_item.get(spelling).is_some());
        // A new table takes the spelling the configuration serializes.
        let chosen = present
            .or_else(|| canonical_channel_url(parent).map(|_| &spellings[spellings.len() - 1]))
            .cloned()
            .unwrap_or_else(|| parent.to_string());
        current_item = current_item.get(&chosen).unwrap_or(&toml_edit::Item::None);
        resolved.push(chosen);
    }

    resolved
}

/// Extract the selected value, wrapped in its parent tables.
fn partial_config(config: &Config, key: &str) -> miette::Result<JsonValue> {
    let key_path = KeyPath::parse(key)?;
    if !is_known_key(config.get_keys(), &key_path) {
        return Err(miette::miette!(
            "unknown key: {}\nSupported keys:\n\t{}",
            console::style(key).red(),
            config.get_keys().join(",\n\t")
        ));
    }

    // TOML omits unset fields instead of serializing them as null.
    let mut value: JsonValue =
        toml_edit::de::from_str(&toml_edit::ser::to_string(config).into_diagnostic()?)
            .into_diagnostic()?;
    for segment in key_path
        .parent_keys
        .iter()
        .map(String::as_str)
        .chain(once(key_path.target()))
    {
        let spelling = key_spellings(segment)
            .into_iter()
            .find(|spelling| value.get(spelling).is_some());
        match spelling.as_deref().and_then(|key| value.get_mut(key)) {
            Some(child) => value = child.take(),
            None => return Ok(JsonValue::Object(Default::default())),
        }
    }

    Ok(key_path
        .parent_keys
        .iter()
        .map(String::as_str)
        .chain(once(key_path.target()))
        .rev()
        .fold(value, |value, segment| {
            JsonValue::Object([(segment.to_string(), value)].into_iter().collect())
        }))
}

/// Match supported keys, with each `<...>` placeholder matching one segment.
fn is_known_key(known_keys: &[&str], key_path: &KeyPath) -> bool {
    known_keys.iter().any(|known| {
        let mut known_segments = known.split('.');
        let matches = key_path
            .parent_keys
            .iter()
            .map(String::as_str)
            .chain(once(key_path.target()))
            .all(|segment| {
                known_segments
                    .next()
                    .is_some_and(|known| known.starts_with('<') || known == segment)
            });
        matches && known_segments.next().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_config::DetectorDecision;

    struct TestContext {
        pub config_path: PathBuf,
        pub common_args: CommonArgs,
        pub _temp_dir: tempfile::TempDir,
    }

    impl TestContext {
        fn read_config(&self) -> String {
            let config_read_result = fs_err::read_to_string(&self.config_path);

            config_read_result.expect("Should be able to read the config file after update")
        }

        fn setup(config_content: Option<&str>) -> Self {
            let temp_dir = tempfile::tempdir().unwrap();
            let project_root = temp_dir.path();

            let config_path = project_root.join("config.toml");
            fs_err::write(&config_path, config_content.unwrap_or("")).unwrap();

            let common_args = CommonArgs {
                local: false,
                global: false,
                system: false,
                shared: false,
                path: Some(config_path.clone()),
                workspace_config: WorkspaceConfig {
                    manifest_path: Some(temp_dir.path().to_path_buf()),
                    ..Default::default()
                },
            };

            Self {
                _temp_dir: temp_dir,
                common_args,
                config_path,
            }
        }
    }

    async fn execute_subcommand(subcommand: Subcommand) {
        let args = Args { subcommand };
        let result = execute(args).await;

        result.expect("The subcommand execution failed");
    }

    #[tokio::test]
    async fn channel_consent_round_trips_across_url_spellings() {
        let context = TestContext::setup(Some("default-channels = [\"conda-forge\"]\n"));
        let origin = ChannelUrl::from(url::Url::parse("https://prefix.dev/conda-forge").unwrap());
        let typed = "virtual-package-detectors.consent.\"https://prefix.dev/conda-forge/\"";
        alter_config(
            &context.common_args,
            typed,
            Some("allow".to_string()),
            AlterMode::Set,
        )
        .await
        .unwrap();
        let config = Config::from_path(&context.config_path).unwrap();
        assert_eq!(
            config.virtual_package_detectors.consent(&origin),
            Some(DetectorDecision::Allow)
        );
        assert_eq!(
            partial_config(&config, typed).unwrap(),
            serde_json::json!({
                "virtual-package-detectors": {
                    "consent": {"https://prefix.dev/conda-forge/": "allow"}
                }
            })
        );
        alter_config(
            &context.common_args,
            "virtual-package-detectors.consent.\"https://prefix.dev/conda-forge\"",
            Some("deny".to_string()),
            AlterMode::Set,
        )
        .await
        .unwrap();
        let config = Config::from_path(&context.config_path).unwrap();
        assert_eq!(
            config.virtual_package_detectors.consent(&origin),
            Some(DetectorDecision::Deny)
        );
        assert_eq!(config.virtual_package_detectors.consent.len(), 1);
        alter_config(&context.common_args, typed, None, AlterMode::Unset)
            .await
            .unwrap();
        let config = Config::from_path(&context.config_path).unwrap();
        assert_eq!(config.virtual_package_detectors.consent(&origin), None);
        assert_eq!(
            config.default_channels,
            vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]
        );
    }

    #[tokio::test]
    async fn empty_detector_config_revokes_consent() {
        let origin = ChannelUrl::from(url::Url::parse("https://prefix.dev/conda-forge").unwrap());
        for key in [
            "virtual-package-detectors.consent",
            "virtual-package-detectors",
        ] {
            let context = TestContext::setup(Some(
                r#"default-channels = ["conda-forge"]
[virtual-package-detectors.consent]
"https://prefix.dev/conda-forge" = "allow"
"#,
            ));
            alter_config(
                &context.common_args,
                key,
                Some("{}".to_string()),
                AlterMode::Set,
            )
            .await
            .unwrap();
            let config = Config::from_path(&context.config_path).unwrap();
            assert_eq!(config.virtual_package_detectors.consent(&origin), None);
            assert_eq!(
                config.default_channels,
                vec![NamedChannelOrUrl::from_str("conda-forge").unwrap()]
            );
        }
    }

    #[tokio::test]
    async fn repository_consent_edits_follow_the_destination() {
        for (local_scope, explicit_path) in [(true, false), (false, false), (false, true)] {
            local_consent_edits_preserve_unselected_channels(local_scope, explicit_path).await;
        }
    }

    async fn local_consent_edits_preserve_unselected_channels(
        local_scope: bool,
        explicit_path: bool,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        let config_home = directory.path().join("user-config");
        fs_err::create_dir_all(&repository).unwrap();
        fs_err::write(
            repository.join("pixi.toml"),
            "[workspace]\nname = \"consent-test\"\nchannels = []\n",
        )
        .unwrap();
        let args = CommonArgs {
            local: local_scope,
            global: false,
            system: false,
            shared: false,
            path: explicit_path
                .then(|| repository.join(consts::PIXI_DIR).join(consts::CONFIG_FILE)),
            workspace_config: WorkspaceConfig {
                manifest_path: Some(repository.join("pixi.toml")),
                ..Default::default()
            },
        };
        let first = ChannelUrl::from(url::Url::parse("https://prefix.dev/first").unwrap());
        let second = ChannelUrl::from(url::Url::parse("https://prefix.dev/second").unwrap());
        let local = repository.join(consts::PIXI_DIR).join(consts::CONFIG_FILE);
        temp_env::async_with_vars(
            [
                ("XDG_CONFIG_HOME", Some(config_home.as_os_str())),
                ("APPDATA", Some(config_home.as_os_str())),
            ],
            async {
                pixi_config::write_detector_decision(&local, &first, DetectorDecision::Allow)
                    .unwrap();
                pixi_config::write_detector_decision(&local, &second, DetectorDecision::Allow)
                    .unwrap();
                alter_config(
                    &args,
                    "virtual-package-detectors.timeout-seconds",
                    Some("10".to_string()),
                    AlterMode::Set,
                )
                .await
                .unwrap();
                let source = GlobalConfigSource::None;
                let config = Config::load_with(&repository, &source);
                assert_eq!(
                    config.virtual_package_detectors.consent(&first),
                    Some(DetectorDecision::Allow)
                );
                assert_eq!(
                    config.virtual_package_detectors.consent(&second),
                    Some(DetectorDecision::Allow)
                );
                let key = "virtual-package-detectors.consent.\"https://prefix.dev/first/\"";
                alter_config(&args, key, Some("deny".to_string()), AlterMode::Set)
                    .await
                    .unwrap();
                let config = Config::load_with(&repository, &source);
                assert_eq!(
                    config.virtual_package_detectors.consent(&first),
                    Some(DetectorDecision::Deny)
                );
                assert_eq!(
                    config.virtual_package_detectors.consent(&second),
                    Some(DetectorDecision::Allow)
                );
                alter_config(&args, key, None, AlterMode::Unset)
                    .await
                    .unwrap();
                assert_eq!(
                    Config::load_with(&repository, &source)
                        .virtual_package_detectors
                        .consent(&first),
                    None
                );
                pixi_config::write_detector_decision(&local, &first, DetectorDecision::Allow)
                    .unwrap();
                assert_eq!(
                    Config::load_with(&repository, &source)
                        .virtual_package_detectors
                        .consent(&first),
                    Some(DetectorDecision::Allow)
                );
                alter_config(
                    &args,
                    "virtual-package-detectors.consent",
                    Some(r#"{"https://prefix.dev/second/": "allow"}"#.to_string()),
                    AlterMode::Set,
                )
                .await
                .unwrap();
                let config = Config::load_with(&repository, &source);
                assert_eq!(config.virtual_package_detectors.consent(&first), None);
                assert_eq!(
                    config.virtual_package_detectors.consent(&second),
                    Some(DetectorDecision::Allow)
                );
                alter_config(
                    &args,
                    "virtual-package-detectors",
                    Some(r#"{"timeout-seconds": 20}"#.to_string()),
                    AlterMode::Set,
                )
                .await
                .unwrap();
                assert_eq!(
                    Config::load_with(&repository, &source)
                        .virtual_package_detectors
                        .consent(&second),
                    None
                );
            },
        )
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repository_consent_edits_cannot_modify_linked_external_configs() {
        for link_kind in ["directory", "file", "hardlink"] {
            for (local_scope, explicit_path) in [(true, false), (false, false), (false, true)] {
                let directory = tempfile::tempdir().unwrap();
                let repository = directory.path().join("repository");
                let external = directory.path().join("external");
                let config_home = directory.path().join("user-config");
                fs_err::create_dir_all(&repository).unwrap();
                fs_err::create_dir_all(&external).unwrap();
                fs_err::write(
                    repository.join("pixi.toml"),
                    "[workspace]\nname = \"consent-test\"\nchannels = []\n",
                )
                .unwrap();
                let target = external.join(consts::CONFIG_FILE);
                let original = "offline = true\n";
                fs_err::write(&target, original).unwrap();
                let local_directory = repository.join(consts::PIXI_DIR);
                let local = local_directory.join(consts::CONFIG_FILE);
                if link_kind == "directory" {
                    std::os::unix::fs::symlink(&external, &local_directory).unwrap();
                } else {
                    fs_err::create_dir(&local_directory).unwrap();
                    if link_kind == "file" {
                        std::os::unix::fs::symlink(&target, &local).unwrap();
                    } else {
                        fs_err::hard_link(&target, &local).unwrap();
                    }
                }
                let args = CommonArgs {
                    local: local_scope,
                    global: false,
                    system: false,
                    shared: false,
                    path: explicit_path.then_some(local),
                    workspace_config: WorkspaceConfig {
                        manifest_path: Some(repository.join("pixi.toml")),
                        ..Default::default()
                    },
                };
                temp_env::async_with_vars(
                    [
                        ("XDG_CONFIG_HOME", Some(config_home.as_os_str())),
                        ("APPDATA", Some(config_home.as_os_str())),
                    ],
                    async {
                        assert!(
                            alter_config(
                                &args,
                                "virtual-package-detectors.consent.\"https://prefix.dev/first\"",
                                Some("allow".to_string()),
                                AlterMode::Set,
                            )
                            .await
                            .is_err(),
                            "{link_kind}, local={local_scope}, explicit={explicit_path}"
                        );
                        assert_eq!(fs_err::read_to_string(&target).unwrap(), original);
                    },
                )
                .await;
            }
        }
    }

    #[tokio::test]
    async fn test_determine_config_write_path() {
        let test_context = TestContext::setup(None);
        let mut config_path = test_context.config_path.clone();

        let mut config_write_path = determine_config_write_path(&test_context.common_args)
            .await
            .expect("Determine config write path should have succeeded");

        if cfg!(target_os = "macos") {
            config_path = config_path
                .canonicalize()
                .expect("Failed to canonicalize temp directory path");

            config_write_path = config_write_path
                .canonicalize()
                .expect("Failed to canonicalize temp directory path");
        }

        assert_eq!(config_write_path, config_path);
    }

    fn shared_args() -> CommonArgs {
        CommonArgs {
            local: false,
            global: false,
            system: false,
            shared: true,
            path: None,
            workspace_config: WorkspaceConfig::default(),
        }
    }

    /// `--shared` writes the user-level rattler file and only accepts keys
    /// every rattler-based tool understands.
    #[tokio::test]
    async fn shared_writes_the_rattler_file_and_refuses_pixi_keys() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_home = temp_dir.path().join("config-home");
        let rattler_home = config_home.join("rattler");
        fs_err::create_dir_all(&rattler_home).unwrap();
        fs_err::write(rattler_home.join("config.toml"), "").unwrap();
        temp_env::async_with_vars(
            [
                ("XDG_CONFIG_HOME", Some(config_home.to_str().unwrap())),
                ("RATTLER_HOME", Some(rattler_home.to_str().unwrap())),
            ],
            async {
                let args = shared_args();
                let path = determine_config_write_path(&args).await.unwrap();
                assert_eq!(path, rattler_home.join("config.toml"));

                alter_config(
                    &args,
                    "virtual-package-detectors.consent.\"https://conda.anaconda.org/conda-forge\"",
                    Some("allow".to_string()),
                    AlterMode::Set,
                )
                .await
                .unwrap();
                let origin = ChannelUrl::from(
                    url::Url::parse("https://conda.anaconda.org/conda-forge").unwrap(),
                );
                assert_eq!(
                    Config::from_shared_path(&path)
                        .unwrap()
                        .virtual_package_detectors
                        .consent(&origin),
                    Some(DetectorDecision::Allow)
                );

                assert!(
                    alter_config(
                        &args,
                        "shell.change-ps1",
                        Some("false".to_string()),
                        AlterMode::Set,
                    )
                    .await
                    .is_err()
                );
                assert_eq!(
                    Config::from_shared_path(&path)
                        .unwrap()
                        .virtual_package_detectors
                        .consent(&origin),
                    Some(DetectorDecision::Allow)
                );

                alter_config(
                    &args,
                    "virtual-package-detectors.consent.\"https://conda.anaconda.org/conda-forge\"",
                    None,
                    AlterMode::Unset,
                )
                .await
                .unwrap();
                assert_eq!(
                    Config::from_shared_path(&path)
                        .unwrap()
                        .virtual_package_detectors
                        .consent(&origin),
                    None
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn set_creates_missing_pixi_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project_root = temp_dir.path();

        fs_err::write(
            project_root.join("pixi.toml"),
            r#"[workspace]
            name = "test-workspace"
            channels = []"#,
        )
        .unwrap();

        let common_args = CommonArgs {
            local: false,
            global: false,
            system: false,
            shared: false,
            path: None,
            workspace_config: WorkspaceConfig {
                manifest_path: Some(temp_dir.path().to_path_buf()),
                ..Default::default()
            },
        };

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "pinning-strategy".to_owned(),
            value: Some("semver".to_owned()),
            common: common_args,
        }))
        .await;

        // Assert that the file AND the directory now exist
        let config_path = project_root.join(".pixi/config.toml");
        assert!(config_path.exists(), "Config file should have been created");
        assert!(
            config_path.parent().unwrap().exists(),
            "Parent directory should have been created"
        );
    }

    #[tokio::test]
    async fn list_empty_config() {
        let test_context = TestContext::setup(None);

        execute_subcommand(Subcommand::List(ListArgs {
            key: None,
            json: false,
            common: test_context.common_args,
            config_source: pixi_config::ConfigSourceCli::default(),
        }))
        .await;
    }

    #[tokio::test]
    async fn set_valid_key() {
        let test_context = TestContext::setup(None);

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "pinning-strategy".to_owned(),
            value: Some("semver".to_owned()),
            common: test_context.common_args,
        }))
        .await;
    }

    #[tokio::test]
    async fn set_preserves_comments() {
        let test_context = TestContext::setup(Some(
            "# some comment which should be kept\nallow-symbolic-links = true",
        ));

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "tls-no-verify".to_owned(),
            value: Some("false".to_owned()),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @"
        # some comment which should be kept
        allow-symbolic-links = true
        tls-no-verify = false
        "
        );
    }

    #[tokio::test]
    async fn set_non_existent_key_to_none() {
        let test_context = TestContext::setup(Some("allow-symbolic-links = true"));

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "tls-no-verify".to_owned(),
            value: None,
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @"allow-symbolic-links = true"
        );
    }

    #[tokio::test]
    async fn set_existing_key_to_none_removes_key() {
        let test_context = TestContext::setup(Some("allow-symbolic-links = true"));

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "allow-symbolic-links".to_owned(),
            value: None,
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @""
        );
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn set_table_creation() {
        let test_context = TestContext::setup(Some("allow-symbolic-links = true"));

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "cache.root".to_owned(),
            value: Some("/tmp/pixi-cache".to_owned()),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"
        allow-symbolic-links = true

        [cache]
        root = "/tmp/pixi-cache"
        "#
        );
    }

    #[tokio::test]
    async fn unset_dotted_key() {
        let test_context = TestContext::setup(Some(
            r#"[repodata-config."https://prefix.dev"]
disable-sharded = false"#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: "repodata-config.\"https://prefix.dev\".disable-sharded".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @""
        );
    }

    #[tokio::test]
    async fn unset_missing_key() {
        let test_context = TestContext::setup(None);

        let args = Args {
            subcommand: Subcommand::Unset(UnsetArgs {
                key: "pinning-strategy".to_owned(),
                common: test_context.common_args,
            }),
        };
        let result = execute(args).await;

        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found in configuration file"));
    }

    #[tokio::test]
    async fn unset_on_existing_stale_key() {
        let test_context = TestContext::setup(Some(
            r#"[shell]
stale-key = "some_value"
not-stale-key = "another_value"
            "#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: "shell.stale-key".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"
        [shell]
        not-stale-key = "another_value"
        "#,
        );
    }

    #[tokio::test]
    async fn unset_on_stale_key_removes_empty_parent_table() {
        let test_context = TestContext::setup(Some(
            r#"[shell]
            stale-key = "some_value"
            "#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: "shell.stale-key".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @""
        );
    }

    #[tokio::test]
    async fn unset_key_with_sibling_kept() {
        let test_context = TestContext::setup(Some(
            r#"[repodata-config."https://backup.example.com"]
disable-zstd = true

[repodata-config."https://primary.example.com"]
disable-sharded = false
"#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: r#"repodata-config."https://backup.example.com".disable-zstd"#.to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"

        [repodata-config."https://primary.example.com"]
        disable-sharded = false
        "#
        );
    }

    #[tokio::test]
    async fn unset_key_removes_nested_empty_parent_table() {
        let test_context = TestContext::setup(Some(
            r#"[pypi-options]
index-url = "https://pypi.org/simple"
"#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: "pypi-options.index-url".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @""
        );
    }

    #[tokio::test]
    async fn unset_config_key_keeps_its_comment() {
        let test_context = TestContext::setup(Some(
            r#"# some comment that is being kept from the deleted key
allow-symbolic-links = true
stale-key = "some-value"
stale-key2 = "some-other-value"
        "#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: "allow-symbolic-links".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"
        # some comment that is being kept from the deleted key
        stale-key = "some-value"
        stale-key2 = "some-other-value"
        "#
        );
    }

    #[tokio::test]
    async fn append_single_line() {
        let test_context = TestContext::setup(Some(
            r#"allow-symbolic-links = true
default-channels = ["conda-forge"]
"#,
        ));

        execute_subcommand(Subcommand::Append(PendArgs {
            key: "default-channels".to_owned(),
            value: "new-channel".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"
        allow-symbolic-links = true
        default-channels = ["conda-forge", "new-channel"]
        "#
        );
    }

    #[tokio::test]
    async fn append_multi_line() {
        let test_context = TestContext::setup(Some(
            r#"allow-symbolic-links = true
default-channels = [
    "conda-forge",
]
        "#,
        ));

        execute_subcommand(Subcommand::Append(PendArgs {
            key: "default-channels".to_owned(),
            value: "new-channel".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"
        allow-symbolic-links = true
        default-channels = [
            "conda-forge",
            "new-channel",
        ]
        "#
        );
    }

    #[tokio::test]
    async fn append_from_scratch() {
        let test_context = TestContext::setup(None);

        execute_subcommand(Subcommand::Append(PendArgs {
            key: "default-channels".to_owned(),
            value: "new-channel".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"default-channels = ["new-channel"]"#
        );
    }

    #[tokio::test]
    async fn prepend_single_line() {
        let test_context = TestContext::setup(Some(
            r#"allow-symbolic-links = true
default-channels = ["conda-forge"]
"#,
        ));

        execute_subcommand(Subcommand::Prepend(PendArgs {
            key: "default-channels".to_owned(),
            value: "new-channel".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"
        allow-symbolic-links = true
        default-channels = ["new-channel", "conda-forge"]
        "#
        );
    }

    #[tokio::test]
    async fn prepend_multi_line() {
        let test_context = TestContext::setup(Some(
            r#"allow-symbolic-links = true
default-channels = [
    "conda-forge",
]
        "#,
        ));

        execute_subcommand(Subcommand::Prepend(PendArgs {
            key: "default-channels".to_owned(),
            value: "new-channel".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"
        allow-symbolic-links = true
        default-channels = [
            "new-channel",
            "conda-forge",
        ]
        "#
        );
    }

    #[tokio::test]
    async fn prepend_from_scratch() {
        let test_context = TestContext::setup(None);

        execute_subcommand(Subcommand::Prepend(PendArgs {
            key: "default-channels".to_owned(),
            value: "new-channel".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"default-channels = ["new-channel"]"#
        );
    }

    #[tokio::test]
    async fn set_kebab_case_overwrites_legacy_snake_case_key() {
        let test_context =
            TestContext::setup(Some("other = 1\n# keep this comment\ntls_no_verify = true"));

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "tls-no-verify".to_owned(),
            value: Some("false".to_owned()),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @"
        other = 1
        # keep this comment
        tls-no-verify = false
        "
        );

        // Verify subsequent modifications on the newly canonicalized key work cleanly
        execute_subcommand(Subcommand::Set(SetArgs {
            key: "tls-no-verify".to_owned(),
            value: Some("true".to_owned()),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @"
        other = 1
        # keep this comment
        tls-no-verify = true
        "
        );
    }

    #[tokio::test]
    async fn set_nested_key_reuses_snake_case_parent_table() {
        let test_context = TestContext::setup(Some(
            r#"
[repodata_config]
disable-sharded = true
"#,
        ));

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "repodata-config.disable-sharded".to_owned(),
            common: test_context.common_args.clone(),
            value: Some("false".to_owned()),
        }))
        .await;

        insta::assert_snapshot!(
                    test_context.read_config(),
                    @"

        [repodata_config]
        disable-sharded = false
        ",
        );
    }

    #[tokio::test]
    async fn set_nested_key_overwrites_legacy_snake_case_child() {
        let test_context = TestContext::setup(Some(
            r#"
[repodata-config]
disable_bzip2 = true
"#,
        ));

        execute_subcommand(Subcommand::Set(SetArgs {
            key: "repodata-config.disable-bzip2".to_owned(),
            common: test_context.common_args.clone(),
            value: Some("false".to_owned()),
        }))
        .await;

        insta::assert_snapshot!(
                    test_context.read_config(),
                    @"

        [repodata-config]
        disable-bzip2 = false
        ",
        );
    }

    #[tokio::test]
    async fn append_preserves_snake_case_key() {
        let test_context = TestContext::setup(Some(
            r#"
default_channels = ["conda-forge"]
"#,
        ));

        execute_subcommand(Subcommand::Append(PendArgs {
            key: "default-channels".to_owned(),
            value: "new-channel".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"

        default_channels = ["conda-forge", "new-channel"]
        "#,
        );
    }

    #[tokio::test]
    async fn prepend_preserves_snake_case_key() {
        let test_context = TestContext::setup(Some(
            r#"
default_channels = ["conda-forge"]
"#,
        ));

        execute_subcommand(Subcommand::Prepend(PendArgs {
            key: "default-channels".to_owned(),
            value: "new-channel".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
            test_context.read_config(),
            @r#"

        default_channels = ["new-channel", "conda-forge"]
        "#,
        );
    }

    #[tokio::test]
    async fn unset_snake_case_target_key() {
        let test_context = TestContext::setup(Some(
            r#"
tls_no_verify = true
"#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: "tls-no-verify".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
                    test_context.read_config(),
                    @"",
        );
    }

    #[tokio::test]
    async fn unset_nested_snake_case_child_key() {
        let test_context = TestContext::setup(Some(
            r#"
[repodata_config]
disable_sharded = true
disable-shared = true
"#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: "repodata-config.disable-sharded".to_owned(),
            common: test_context.common_args.clone(),
        }))
        .await;

        insta::assert_snapshot!(
                    test_context.read_config(),
                    @ "

        [repodata_config]
        disable-shared = true
        ",
        );
    }

    #[tokio::test]
    async fn unset_invalidating_config_fails() {
        let test_context = TestContext::setup(Some(
            r#"
[s3-options.bucket]
endpoint-url = "https://my-s3-compatible-host.com"
addressing-style = "path"
region = "us-east-1"
"#,
        ));

        let args = Args {
            subcommand: Subcommand::Unset(UnsetArgs {
                key: "s3-options.bucket.region".to_owned(),
                common: test_context.common_args.clone(),
            }),
        };

        let result = execute(args).await;
        let err = result
            .expect_err("expected unset on required field to return an error, but it succeeded ");
        assert!(
            err.to_string()
                .contains("would leave the config file invalid")
                || err.to_string().contains("missing field `region`")
        );
    }

    #[test]
    fn test_partial_config_nested_keys() {
        let (config, _) = Config::from_toml(
            r#"
default-channels = ["conda-forge"]

[pypi-config]
index-url = "https://pypi.example.com/simple"
extra-index-urls = ["https://extra.example.com/simple"]

[s3-options."my.bucket"]
endpoint-url = "https://s3.example.com"
region = "eu-west-1"
addressing-style = "path"

[virtual-package-detectors]
timeout-seconds = 7
"#,
            None,
        )
        .unwrap();
        assert_eq!(
            partial_config(&config, "pypi-config.index-url").unwrap(),
            serde_json::json!({
                "pypi-config": {"index-url": "https://pypi.example.com/simple"}
            })
        );
        assert_eq!(
            partial_config(&config, r#"s3-options."my.bucket".region"#).unwrap(),
            serde_json::json!({"s3-options": {"my.bucket": {"region": "eu-west-1"}}})
        );
        assert_eq!(
            partial_config(&config, "virtual-package-detectors.timeout-seconds").unwrap(),
            serde_json::json!({"virtual-package-detectors": {"timeout-seconds": 7}})
        );
    }

    #[test]
    fn test_partial_config_unset_and_unknown_keys() {
        let (config, _) = Config::from_toml("default-channels = [\"conda-forge\"]", None).unwrap();
        assert_eq!(
            partial_config(&config, "s3-options.missing.region").unwrap(),
            serde_json::json!({})
        );
        for key in [
            "not-a-key",
            "offline.nested",
            "s3-options.missing.not-a-field",
        ] {
            assert!(partial_config(&config, key).is_err(), "{key}");
        }
    }
}
