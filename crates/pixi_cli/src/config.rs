use crate::cli_config::WorkspaceConfig;
use clap::Parser;
use miette::{IntoDiagnostic, WrapErr};
use pixi_config;
use pixi_config::Config;
use pixi_consts::consts;
use pixi_core::WorkspaceLocator;
use pixi_core::workspace::WorkspaceLocatorError;
use pixi_manifest::toml::TomlDocument;
use pixi_toml_edit::{insert_array_element, push_array_element, remove_entry, upsert_entry};
use rattler_conda_types::NamedChannelOrUrl;
use std::{io::Write, path::PathBuf, str::FromStr};
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
    let to = determine_config_write_path(common_args)?;
    let content = match fs_err::read_to_string(&to) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e)
                .into_diagnostic()
                .context("failed to read config file");
        }
    };

    let doc_mut = content
        .parse::<toml_edit::DocumentMut>()
        .into_diagnostic()
        .context("failed to parse TOML")?;

    let mut toml_doc = TomlDocument::new(doc_mut);

    let mut config = load_config(common_args)?;

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

            transplant_config_key(&config, &mut toml_doc, key, &mode)?;
        }
        AlterMode::Set => {
            // Run set on Config object for validation
            config.set(key, value)?;

            transplant_config_key(&config, &mut toml_doc, key, &mode)?;
        }
        AlterMode::Unset => unset(&mut toml_doc, key)?,
    }

    let contents = toml_doc.to_string();
    let parent = to.parent().expect("config path should have a parent");
    fs_err::create_dir_all(parent)
        .into_diagnostic()
        .wrap_err(format!(
            "failed to create directories in '{}'",
            parent.display()
        ))?;
    fs_err::write(&to, contents)
        .into_diagnostic()
        .wrap_err(format!("failed to write config to '{}'", to.display()))?;

    eprintln!("✅ Updated config at {}", to.display());
    Ok(())
}

fn unset(toml_doc: &mut TomlDocument, key: &str) -> miette::Result<()> {
    let key_path = KeyPath::parse(key)?;

    let parent_table = if key_path.parents().is_empty() {
        toml_doc.as_item_mut()
    } else {
        toml_doc
            .get_or_insert_nested_item(&key_path.parents())
            .into_diagnostic()?
    };

    let removed = remove_entry(parent_table, key_path.target()).into_diagnostic()?;

    if removed.is_none() {
        return Err(miette::miette!(
            "Key '{}' not found in configuration file",
            key
        ));
    }

    Ok(())
}

fn transplant_config_key(
    config: &Config,
    toml_doc: &mut TomlDocument,
    key: &str,
    mode: &AlterMode,
) -> miette::Result<()> {
    // We serialize the entire Config and parse it into a temporary document because:
    // 1. The input value undergoes strict type validation via Serde.
    // 2. We extract only the specific target leaf node, preventing unrequested default values.
    let key_path = KeyPath::parse(key)?;

    let full_serialized = toml_edit::ser::to_string(&config).into_diagnostic()?;
    let temp_doc = full_serialized.parse::<DocumentMut>().into_diagnostic()?;

    // walk down all the way to the leaf
    let mut current_item = temp_doc.as_item();
    for part in key.split('.') {
        current_item = current_item.get(part).unwrap_or(&Item::None);
    }

    if current_item.is_none() {
        // fall back into unset
        match unset(toml_doc, key) {
            Ok(()) => return Ok(()),
            Err(e) if e.to_string().contains("not found in configuration file") => return Ok(()),
            Err(e) => return Err(e),
        }
    }

    let target_table = if key_path.parents().is_empty() {
        Ok(toml_doc.as_item_mut())
    } else {
        toml_doc
            .get_or_insert_nested_item(&key_path.parents())
            .into_diagnostic()
    }?;

    if let AlterMode::Append | AlterMode::Prepend = mode
        && let Some(new_value) = current_item.as_value()
        && let Some(serialized_array) = new_value.as_array()
    {
        let target_array = toml_doc
            .get_or_insert_toml_array_mut(&key_path.parents(), key_path.target())
            .into_diagnostic()?;

        if matches!(mode, AlterMode::Prepend) {
            if let Some(new_item) = serialized_array.get(0) {
                insert_array_element(target_array, 0, new_item.clone());
            }
        } else {
            if let Some(new_item) = serialized_array.iter().last() {
                push_array_element(target_array, new_item.clone());
            }
        }
        return Ok(());
    }

    if let Some(value) = current_item.as_value() {
        upsert_entry(target_table, key, value.clone()).into_diagnostic()?;
    } else if let Some(table_to_insert) = current_item.as_table()
        && let Some(table_like) = target_table.as_table_like_mut()
    {
        table_like.insert(key_path.target(), Item::Table(table_to_insert.clone()));
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TestContext {
        pub config_path: PathBuf,
        pub common_args: CommonArgs,
        pub temp_dir: tempfile::TempDir,
    }

    impl TestContext {
        fn read_config(&self) -> String {
            let config_read_result = fs_err::read_to_string(&self.config_path);

            config_read_result.expect("Should be able to read the config file after update")
        }

        fn setup(config_content: Option<&str>) -> Self {
            let temp_dir = tempfile::tempdir().unwrap();
            let project_root = temp_dir.path();

            fs_err::create_dir_all(project_root.join(".pixi")).unwrap();
            fs_err::write(
                project_root.join("pixi.toml"),
                r#"[workspace]
            name = "test-workspace"
            channels = []"#,
            )
            .unwrap();

            let config_path = project_root.join(".pixi/config.toml");
            fs_err::write(&config_path, config_content.unwrap_or("")).unwrap();

            let common_args = CommonArgs {
                local: true,
                global: false,
                system: false,
                workspace_config: WorkspaceConfig {
                    manifest_path: Some(temp_dir.path().to_path_buf()),
                    ..Default::default()
                },
            };

            Self {
                temp_dir,
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

    #[test]
    fn determine_project_root_local() {
        let test_context = TestContext::setup(None);
        let mut project_root = test_context.temp_dir.path().to_path_buf();

        if cfg!(target_os = "macos") {
            project_root = project_root
                .canonicalize()
                .expect("Failed to canonicalize temp directory path")
        }

        let project_root_result = determine_project_root(&test_context.common_args)
            .expect("Workspace locator should successfully find the project root");

        assert_eq!(project_root_result, Some(project_root.to_path_buf()));
    }

    #[tokio::test]
    async fn list_empty_config() {
        let test_context = TestContext::setup(None);

        execute_subcommand(Subcommand::List(ListArgs {
            key: None,
            json: false,
            common: test_context.common_args,
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
            "#,
        ));

        execute_subcommand(Subcommand::Unset(UnsetArgs {
            key: "shell.stale-key".to_owned(),
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
            @r#"
        # some comment which should be kept
        allow-symbolic-links = true
        tls-no-verify = false
        "#
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
    async fn unset_config_key_and_its_comment() {
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
}
