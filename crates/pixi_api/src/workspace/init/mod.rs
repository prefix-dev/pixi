use std::{
    collections::HashMap,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use minijinja::{Environment, context};
use pixi_config::{Config, S3OptionsMap, get_default_author, pixi_home};
use pixi_consts::consts;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::{
    CondaPypiMap, CondaPypiMapEntry, CondaPypiMapSpec, CondaPypiMappingMode, FeatureName,
    pyproject::PyProjectManifest,
};
use pixi_utils::conda_environment_file::CondaEnvFile;
use rattler_conda_types::{NamedChannelOrUrl, Platform};
use same_file::is_same_file;
use tokio::fs::OpenOptions;
use url::Url;
use uv_normalize::PackageName;

use crate::interface::Interface;

mod options;
mod template;

pub use options::{GitAttributes, InitOptions, ManifestFormat};

enum InitStrategy {
    FromEnvFile {
        env_file: PathBuf,
        manifest_path: PathBuf,
    },
    ExtendPyproject {
        manifest_path: PathBuf,
    },
    NewPyproject {
        manifest_path: PathBuf,
    },
    NewPixi {
        manifest_path: PathBuf,
    },
    NewMojo {
        manifest_path: PathBuf,
    },
}

pub struct RenderContext {
    pub default_name: String,
    pub version: String,
    pub author: Option<(String, String)>,
    pub platforms: Vec<String>,
    pub channels: Vec<NamedChannelOrUrl>,
    pub index_url: Option<Url>,
    pub extra_index_urls: Vec<Url>,
    pub s3_options: S3OptionsMap,
    pub conda_pypi_mapping: Option<CondaPypiMap>,
}

fn build_render_context(dir: &Path, options: &InitOptions, config: &Config) -> RenderContext {
    RenderContext {
        default_name: get_name_from_dir(dir).unwrap_or_else(|_| String::from("new_workspace")),
        version: "0.1.0".to_string(),
        author: get_default_author(),
        platforms: resolve_platforms(options),
        channels: resolve_channels_from_options(options, config),
        index_url: config.pypi_config.index_url.clone(),
        extra_index_urls: config.pypi_config.extra_index_urls.clone(),
        s3_options: config.s3_options.clone(),
        conda_pypi_mapping: options.conda_pypi_mapping.clone(),
    }
}

pub async fn init<I: Interface>(interface: &I, options: InitOptions) -> miette::Result<Workspace> {
    // Fail silently if the directory already exists or cannot be created.
    fs_err::create_dir_all(&options.path).into_diagnostic()?;
    let dir = dunce::canonicalize(&options.path).into_diagnostic()?;
    validate_init_directory(&dir, interface).await?;

    let config = Config::load(&dir);
    let render_ctx = build_render_context(&dir, &options, &config);

    let strategy = calculate_strategy(&options, &dir, interface).await?;

    let workspace = match strategy {
        InitStrategy::FromEnvFile {
            env_file,
            manifest_path,
        } => init_from_env_file(interface, manifest_path, &env_file, &config, render_ctx).await?,
        InitStrategy::ExtendPyproject { manifest_path } => {
            extend_pyproject(interface, manifest_path, render_ctx).await?
        }
        InitStrategy::NewPyproject { manifest_path } => {
            create_new_pyproject(interface, manifest_path, render_ctx).await?
        }
        InitStrategy::NewPixi { manifest_path } | InitStrategy::NewMojo { manifest_path } => {
            init_standard_manifest(interface, manifest_path, render_ctx).await?
        }
    };

    create_scm_files(&options, &dir);

    Ok(workspace)
}

async fn calculate_strategy<I: Interface>(
    options: &InitOptions,
    dir: &Path,
    interface: &I,
) -> miette::Result<InitStrategy> {
    let pixi_manifest_path = dir.join(consts::WORKSPACE_MANIFEST);
    let pyproject_manifest_path = dir.join(consts::PYPROJECT_MANIFEST);
    let mojoproject_manifest_path = dir.join(consts::MOJOPROJECT_MANIFEST);

    if let Some(env_file_path) = options.env_file.as_ref() {
        if pixi_manifest_path.is_file() {
            miette::bail!("{} already exists", consts::WORKSPACE_MANIFEST);
        }
        return Ok(InitStrategy::FromEnvFile {
            env_file: env_file_path.clone(),
            manifest_path: pixi_manifest_path,
        });
    }

    let pyproject = should_use_pyproject(options, dir, interface).await?;

    if pyproject && pyproject_manifest_path.is_file() {
        Ok(InitStrategy::ExtendPyproject {
            manifest_path: pyproject_manifest_path,
        })
    } else if pyproject {
        Ok(InitStrategy::NewPyproject {
            manifest_path: pyproject_manifest_path,
        })
    } else if options.format == Some(ManifestFormat::Mojoproject) {
        if mojoproject_manifest_path.is_file() {
            miette::bail!("{} already exists", consts::MOJOPROJECT_MANIFEST);
        }
        Ok(InitStrategy::NewMojo {
            manifest_path: mojoproject_manifest_path,
        })
    } else {
        if pixi_manifest_path.is_file() {
            miette::bail!("{} already exists", consts::WORKSPACE_MANIFEST);
        }
        Ok(InitStrategy::NewPixi {
            manifest_path: pixi_manifest_path,
        })
    }
}

async fn validate_init_directory<I: Interface>(
    init_dir: &Path,
    interface: &I,
) -> miette::Result<()> {
    if is_init_dir_equal_to_pixi_home_parent(init_dir) {
        let help_msg = if interface.is_cli().await {
            format!(
                "Please follow the getting started guide at https://pixi.sh/v{}/init_getting_started/ or run the following command to create a new workspace in a subdirectory:\n\n  {}\n",
                consts::PIXI_VERSION,
                console::style("pixi init my_workspace").bold(),
            )
        } else {
            "You have to select a subdirectory to create a new workspace".to_string()
        };
        miette::bail!(
            help = help_msg,
            "initialization without a name in the parent directory of your PIXI_HOME is not allowed.",
        );
    }

    Ok(())
}

fn is_init_dir_equal_to_pixi_home_parent(init_dir: &Path) -> bool {
    pixi_home()
        .as_ref()
        .and_then(|home_dir| home_dir.parent())
        .and_then(|parent| is_same_file(parent, init_dir).ok())
        .unwrap_or(false)
}

fn resolve_platforms(options: &InitOptions) -> Vec<String> {
    if options.platforms.is_empty() {
        vec![Platform::current().to_string()]
    } else {
        // Dedup so a repeated `--platform` (or one matching the current
        // platform) doesn't write a manifest the parser then rejects.
        options.platforms.iter().cloned().unique().collect()
    }
}

fn resolve_channels_from_options(options: &InitOptions, config: &Config) -> Vec<NamedChannelOrUrl> {
    if let Some(channels) = options.channels.as_ref() {
        channels.clone()
    } else {
        config.default_channels().to_vec()
    }
}

async fn should_use_pyproject<I: Interface>(
    options: &InitOptions,
    dir: &Path,
    interface: &I,
) -> miette::Result<bool> {
    let pixi_manifest_path = dir.join(consts::WORKSPACE_MANIFEST);
    let pyproject_manifest_path = dir.join(consts::PYPROJECT_MANIFEST);

    // Dialog with user to create a 'pyproject.toml' or 'pixi.toml' manifest
    // If nothing is defined but there is a `pyproject.toml` file, ask the user.
    if !pixi_manifest_path.is_file()
        && options.format.is_none()
        && pyproject_manifest_path.is_file()
    {
        interface
            .confirm(&format!(
                "A '{}' file already exists. Do you want to extend it with the '{}' configuration?",
                console::style(consts::PYPROJECT_MANIFEST).bold(),
                console::style("[tool.pixi]").bold().green()
            ))
            .await
    } else {
        Ok(options.format == Some(ManifestFormat::Pyproject))
    }
}

async fn init_from_env_file<I: Interface>(
    interface: &I,
    manifest_path: PathBuf,
    env_file_path: &Path,
    config: &Config,
    render_ctx: RenderContext,
) -> miette::Result<Workspace> {
    let env_file = CondaEnvFile::from_path(env_file_path)?;
    let name = env_file
        .name()
        .unwrap_or(render_ctx.default_name.as_str())
        .to_string();

    let env_vars = env_file.variables();
    // TODO: Improve this:
    //  - Use .condarc as channel config
    let (conda_deps, pypi_deps, channels) = env_file.to_manifest(config)?;

    let env = Environment::new();
    let rendered_workspace_template = render_workspace(
        &env,
        name,
        render_ctx.version.as_ref(),
        render_ctx.author.as_ref(),
        channels,
        render_ctx.platforms.as_ref(),
        render_ctx.index_url.as_ref(),
        render_ctx.extra_index_urls.as_ref(),
        config.s3_options.clone(),
        Some(&env_vars),
        render_ctx.conda_pypi_mapping.as_ref(),
    );
    let mut workspace = WorkspaceMut::from_template(manifest_path, rendered_workspace_template)?;
    workspace.add_specs(conda_deps, pypi_deps, &[], &FeatureName::default())?;
    let workspace = workspace.save().await.into_diagnostic()?;

    interface
        .success(&format!(
            "Created {}",
            // Canonicalize the path to make it more readable, but if it fails just use the path as
            // is.
            workspace.workspace.provenance.path.display()
        ))
        .await;

    Ok(workspace)
}

async fn extend_pyproject<I: Interface>(
    interface: &I,
    manifest_path: PathBuf,
    render_ctx: RenderContext,
) -> miette::Result<Workspace> {
    // Inject a tool.pixi.workspace section into an existing pyproject.toml file if
    // there is one without '[tool.pixi.workspace]'
    let pyproject: PyProjectManifest = PyProjectManifest::from_path(&manifest_path)?;

    // Early exit if 'pyproject.toml' already contains a '[tool.pixi.workspace]' table
    if pyproject.has_pixi_table() {
        interface.info("Nothing to do here: 'pyproject.toml' already contains a '[tool.pixi.workspace]' section.").await;
        let workspace = Workspace::from_path(&manifest_path)?;
        return Ok(workspace);
    }

    let (name, pixi_name) = match pyproject.name() {
        Some(name) => (name.to_string(), false),
        None => (get_pypi_safe_name(&render_ctx.default_name), true),
    };
    let dir = manifest_path.parent().unwrap();
    let environments = pyproject.environments_from_groups(dir).into_diagnostic()?;

    let env = Environment::new();
    let pypi_mapping = render_ctx
        .conda_pypi_mapping
        .as_ref()
        .map(render_conda_pypi_mapping)
        .unwrap_or_default();
    let rv = env
        .render_named_str(
            consts::PYPROJECT_MANIFEST,
            template::PYROJECT_TEMPLATE_EXISTING,
            context! {
                name,
                pixi_name,
                channels => render_ctx.channels,
                platforms => render_ctx.platforms,
                environments,
                pypi_mapping,
                index_url => render_ctx.index_url.as_ref(),
                extra_index_urls => render_ctx.extra_index_urls,
                s3 => relevant_s3_options(render_ctx.s3_options, render_ctx.channels),
            },
        )
        .expect("should be able to render the template");
    if let Err(e) = {
        fs::OpenOptions::new()
            .append(true)
            .open(manifest_path.clone())
            .and_then(|mut p| p.write_all(rv.as_bytes()))
    } {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            manifest_path.to_string_lossy(),
            e
        );
    } else {
        // Inform about the addition of the package itself as an editable dependency of
        // the workspace
        interface
            .success(&format!(
                "Added package '{name}' as an editable dependency."
            ))
            .await;
        // Inform about the addition of environments from optional dependencies
        // or dependency groups (if any)
        if !environments.is_empty() {
            let envs: Vec<&str> = environments.keys().map(AsRef::as_ref).collect();
            interface
                .success(&format!(
                    "Added environment{} '{}' from optional dependencies or dependency groups.",
                    if envs.len() > 1 { "s" } else { "" },
                    envs.join("', '")
                ))
                .await;
        }
    }

    Ok(Workspace::from_path(&manifest_path)?)
}

fn get_pypi_safe_name(name: &str) -> String {
    // PyPI package names must not start or end with '-', '_', or '.'
    // so strip those boundary characters before normalizing.
    let trimmed = name.trim_matches(|c: char| matches!(c, '_' | '-' | '.'));
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed.to_string()
    }
}
async fn create_new_pyproject<I: Interface>(
    interface: &I,
    manifest_path: PathBuf,
    render_ctx: RenderContext,
) -> miette::Result<Workspace> {
    let pypi_safe_name = get_pypi_safe_name(&render_ctx.default_name);

    // Normalize separators to '-' as PyPI dist-info convention requires.
    let pypi_package_name = PackageName::from_str(&pypi_safe_name)
        .map(|name| name.as_dist_info_name().to_string())
        .unwrap_or_else(|_| pypi_safe_name.clone());

    let env = Environment::new();
    let pypi_mapping = render_ctx
        .conda_pypi_mapping
        .as_ref()
        .map(render_conda_pypi_mapping)
        .unwrap_or_default();
    let rv = env
        .render_named_str(
            consts::PYPROJECT_MANIFEST,
            template::NEW_PYROJECT_TEMPLATE,
            context! {
                name => pypi_safe_name,
                pypi_package_name,
                version => render_ctx.version,
                author => render_ctx.author,
                channels=> render_ctx.channels,
                platforms=> render_ctx.platforms,
                pypi_mapping,
                index_url => render_ctx.index_url.as_ref(),
                extra_index_urls => &render_ctx.extra_index_urls,
                s3 => relevant_s3_options(render_ctx.s3_options, render_ctx.channels),
            },
        )
        .expect("should be able to render the template");
    save_manifest_file(interface, &manifest_path, rv).await?;

    let dir = manifest_path.parent().unwrap();
    let src_dir = dir.join("src").join(pypi_package_name);
    tokio::fs::create_dir_all(&src_dir)
        .await
        .into_diagnostic()
        .wrap_err_with(|| format!("Could not create directory {}.", src_dir.display()))?;

    let init_file = src_dir.join("__init__.py");
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&init_file)
        .await
    {
        Ok(_) => (),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            // If the file already exists, do nothing
        }
        Err(e) => {
            return Err(e)
                .into_diagnostic()
                .wrap_err_with(|| format!("Could not create file {}.", init_file.display()));
        }
    };

    Ok(Workspace::from_path(&manifest_path)?)
}

async fn init_standard_manifest<I: Interface>(
    interface: &I,
    manifest_path: PathBuf,
    render_ctx: RenderContext,
) -> miette::Result<Workspace> {
    // Create a 'pixi.toml' manifest
    let env = Environment::new();
    let rv = render_workspace(
        &env,
        render_ctx.default_name,
        &render_ctx.version,
        render_ctx.author.as_ref(),
        render_ctx.channels,
        &render_ctx.platforms,
        render_ctx.index_url.as_ref(),
        &render_ctx.extra_index_urls,
        render_ctx.s3_options,
        None,
        render_ctx.conda_pypi_mapping.as_ref(),
    );
    save_manifest_file(interface, &manifest_path, rv).await?;
    Ok(Workspace::from_path(&manifest_path)?)
}

#[allow(clippy::too_many_arguments)]
fn render_workspace(
    env: &Environment<'_>,
    name: String,
    version: &str,
    author: Option<&(String, String)>,
    channels: Vec<NamedChannelOrUrl>,
    platforms: &Vec<String>,
    index_url: Option<&Url>,
    extra_index_urls: &Vec<Url>,
    s3_options: S3OptionsMap,
    env_vars: Option<&HashMap<String, String>>,
    pypi_mapping: Option<&CondaPypiMap>,
) -> String {
    let ctx = context! {
        name,
        version,
        author,
        channels,
        platforms,
        index_url,
        extra_index_urls,
        s3 => relevant_s3_options(s3_options, channels),
        env_vars => {if let Some(env_vars) = env_vars {
            env_vars.iter().map(|(k, v)| format!("{k} = \"{v}\"")).collect::<Vec<String>>().join(", ")
        } else {String::new()}},
        pypi_mapping => pypi_mapping.map(render_conda_pypi_mapping).unwrap_or_default(),
    };

    env.render_named_str(
        consts::WORKSPACE_MANIFEST,
        template::WORKSPACE_TEMPLATE,
        ctx,
    )
    .expect("should be able to render the template")
}

fn render_conda_pypi_mapping(mapping: &CondaPypiMap) -> String {
    match mapping {
        CondaPypiMap::Disabled => "false".to_string(),
        CondaPypiMap::Map(map) => {
            let entries = map
                .iter()
                .sorted_by_key(|(channel, _)| channel.to_string())
                .map(|(channel, entry)| {
                    format!(
                        "{} = {}",
                        quote_toml_string(&channel.to_string()),
                        render_conda_pypi_map_entry(entry)
                    )
                })
                .join(", ");
            format!("{{ {entries} }}")
        }
    }
}

fn render_conda_pypi_map_entry(entry: &CondaPypiMapEntry) -> String {
    match entry {
        CondaPypiMapEntry::Disabled => "false".to_string(),
        CondaPypiMapEntry::Map(spec) => render_conda_pypi_map_spec(spec),
    }
}

fn render_conda_pypi_map_spec(spec: &CondaPypiMapSpec) -> String {
    if let CondaPypiMapSpec {
        location: Some(location),
        mapping: None,
        mapping_mode: CondaPypiMappingMode::Overlay,
        same_name_heuristic: None,
    } = spec
    {
        return quote_toml_string(location);
    }

    let mut fields = Vec::new();
    if let Some(location) = &spec.location {
        fields.push(format!("location = {}", quote_toml_string(location)));
    }
    if let Some(mapping) = &spec.mapping {
        fields.push(format!("mapping = {}", render_inline_pypi_mapping(mapping)));
    }
    if spec.mapping_mode != CondaPypiMappingMode::Overlay || fields.is_empty() {
        fields.push(format!(
            "mapping-mode = {}",
            quote_toml_string(&spec.mapping_mode.to_string())
        ));
    }
    if let Some(same_name_heuristic) = spec.same_name_heuristic {
        fields.push(format!("same-name-heuristic = {same_name_heuristic}"));
    }

    format!("{{ {} }}", fields.join(", "))
}

fn render_inline_pypi_mapping(mapping: &HashMap<String, Vec<String>>) -> String {
    let entries = mapping
        .iter()
        .sorted_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(conda_name, pypi_names)| {
            let value = match pypi_names.as_slice() {
                [] => "false".to_string(),
                [name] => quote_toml_string(name),
                names => format!(
                    "[{}]",
                    names.iter().map(|name| quote_toml_string(name)).join(", ")
                ),
            };
            format!("{} = {value}", quote_toml_string(conda_name))
        })
        .join(", ");
    format!("{{ {entries} }}")
}

fn quote_toml_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            ch if ch.is_control() => quoted.push_str(&format!("\\u{:04X}", ch as u32)),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn relevant_s3_options(
    s3_options: S3OptionsMap,
    channels: Vec<NamedChannelOrUrl>,
) -> HashMap<String, pixi_config::S3Options> {
    // only take s3 options in manifest if they are used in the default channels
    let s3_buckets = channels
        .iter()
        .filter_map(|channel| match channel {
            NamedChannelOrUrl::Name(_) => None,
            NamedChannelOrUrl::Path(_) => None,
            NamedChannelOrUrl::Url(url) => {
                if url.scheme() == "s3" {
                    url.host().map(|host| host.to_string())
                } else {
                    None
                }
            }
        })
        .collect::<Vec<_>>();

    s3_options
        .0
        .into_iter()
        .filter(|(key, _)| s3_buckets.contains(key))
        .collect()
}

/// Save the rendered template to a file, and print a message to the user.
async fn save_manifest_file<I: Interface>(
    interface: &I,
    path: &Path,
    content: String,
) -> miette::Result<()> {
    fs_err::write(path, content).into_diagnostic()?;
    interface
        .success(&format!(
            "Created {}",
            // Canonicalize the path to make it more readable, but if it fails just use the path as is.
            dunce::canonicalize(path)
                .unwrap_or(path.to_path_buf())
                .display()
        ))
        .await;
    Ok(())
}

fn get_name_from_dir(path: &Path) -> miette::Result<String> {
    Ok(path
        .file_name()
        .ok_or(miette::miette!(
            "Cannot get file or directory name from the path: {}",
            path.to_string_lossy()
        ))?
        .to_string_lossy()
        .to_string())
}

// When the specific template is not in the file or the file does not exist.
// Make the file and append the template to the file.
fn create_or_append_file(path: &Path, template: &str) -> std::io::Result<()> {
    let file = fs_err::read_to_string(path).unwrap_or_default();

    if !file.contains(template) {
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?
            .write_all(template.as_bytes())?;
    }
    Ok(())
}

fn create_scm_files(options: &InitOptions, dir: &Path) {
    let gitignore_path = dir.join(".gitignore");
    let gitattributes_path = dir.join(".gitattributes");
    // create a .gitignore if one is missing
    if let Err(e) =
        create_or_append_file(&gitignore_path, template::GITIGNORE_TEMPLATE.trim_start())
    {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            gitignore_path.to_string_lossy(),
            e
        );
    }

    let git_attributes = options.scm.as_ref().unwrap_or(&GitAttributes::Github);

    // create a .gitattributes if one is missing
    if let Err(e) = create_or_append_file(&gitattributes_path, git_attributes.template()) {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            gitattributes_path.to_string_lossy(),
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Read, path::Path};

    use pixi_manifest::toml::{FromTomlStr, TomlWorkspace};
    use rstest::rstest;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_create_or_append_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file.txt");
        let template = "Test Template";

        fn read_file_content(path: &Path) -> String {
            let mut file = fs_err::File::open(path).unwrap();
            let mut content = String::new();
            file.read_to_string(&mut content).unwrap();
            content
        }

        // Scenario 1: File does not exist.
        create_or_append_file(&file_path, template).unwrap();
        assert_eq!(read_file_content(&file_path), template);

        // Scenario 2: File exists but doesn't contain the template.
        create_or_append_file(&file_path, "New Content").unwrap();
        assert!(read_file_content(&file_path).contains(template));
        assert!(read_file_content(&file_path).contains("New Content"));

        // Scenario 3: File exists and already contains the template.
        let original_content = read_file_content(&file_path);
        create_or_append_file(&file_path, template).unwrap();
        assert_eq!(read_file_content(&file_path), original_content);

        // Scenario 4: Path is a folder not a file, give an error.
        assert!(create_or_append_file(dir.path(), template).is_err());

        dir.close().unwrap();
    }

    fn assert_conda_pypi_map_render_roundtrips(mapping: CondaPypiMap) {
        let rendered = render_conda_pypi_mapping(&mapping);
        let input = format!(
            r#"
            channels = []
            platforms = []
            conda-pypi-map = {rendered}
            "#
        );
        let parsed = TomlWorkspace::from_toml_str(&input)
            .unwrap()
            .conda_pypi_map
            .unwrap();
        assert_eq!(parsed, mapping);
    }

    #[test]
    fn test_render_conda_pypi_map_disabled_roundtrips() {
        assert_conda_pypi_map_render_roundtrips(CondaPypiMap::Disabled);
    }

    #[test]
    fn test_render_conda_pypi_map_full_table_roundtrips() {
        let mut inline_mapping = HashMap::new();
        inline_mapping.insert("not-on-pypi".to_string(), vec![]);
        inline_mapping.insert("pytorch".to_string(), vec!["torch".to_string()]);
        inline_mapping.insert(
            "multi".to_string(),
            vec!["first".to_string(), "second".to_string()],
        );

        let mut map = HashMap::new();
        map.insert(
            NamedChannelOrUrl::from_str("conda-forge").unwrap(),
            CondaPypiMapEntry::Map(CondaPypiMapSpec {
                location: Some("mapping \\\"quoted\\\" dir/map.json".to_string()),
                mapping: Some(inline_mapping),
                mapping_mode: CondaPypiMappingMode::Replace,
                same_name_heuristic: Some(false),
            }),
        );
        map.insert(
            NamedChannelOrUrl::from_str("https://example.com/channel").unwrap(),
            CondaPypiMapEntry::Disabled,
        );

        assert_conda_pypi_map_render_roundtrips(CondaPypiMap::Map(map));
    }

    struct MockInterface {
        pub confirm_response: bool,
    }

    impl Interface for MockInterface {
        async fn is_cli(&self) -> bool {
            false
        }
        async fn confirm(&self, _msg: &str) -> miette::Result<bool> {
            Ok(self.confirm_response)
        }
        async fn error(&self, _msg: &str) {}
        async fn info(&self, _msg: &str) {}
        async fn success(&self, _msg: &str) {}
        async fn warning(&self, _msg: &str) {}
    }

    #[derive(Default)]
    struct TestConfig {
        pub format: Option<ManifestFormat>,
        pub pre_existing_pixi: bool,
        pub pre_existing_pyproject: bool,
        pub pre_existing_mojo: bool,
        pub with_env_file: bool,
        pub confirm_response: bool,
        pub pyproject_already_extended: bool,
    }

    struct TestOutcome {
        pub result: miette::Result<Workspace>,
        pub project_path: std::path::PathBuf,
        pub pixi_exists: bool,
        pub pyproject_exists: bool,
        pub mojo_exists: bool,
        pub _tmp_dir: tempfile::TempDir,
    }

    // Create TestOutcome function
    async fn run_init_scenario(config: TestConfig) -> TestOutcome {
        let tmp_dir = tempfile::tempdir().unwrap();
        let project_path = tmp_dir.path().to_path_buf();

        if config.pre_existing_pyproject {
            let pyproject_path = project_path.join(consts::PYPROJECT_MANIFEST);
            let content = if config.pyproject_already_extended {
                "[workspace]\nname = \"existing_pixi\"\nchannels = []\nplatforms = []\n\n[tool.pixi.workspace]\nchannels = []"
            } else {
                "[workspace]\nname = \"existing_pixi\"\nchannels = []\nplatforms = []"
            };
            fs_err::write(&pyproject_path, content).unwrap();
        }

        if config.pre_existing_pixi {
            let pixi_path = project_path.join(consts::WORKSPACE_MANIFEST);
            fs_err::write(
                &pixi_path,
                "[workspace]\nname = \"existing_pixi\"\nchannels = []\nplatforms = []",
            )
            .unwrap();
        }

        if config.pre_existing_mojo {
            let mojo_path = project_path.join(consts::MOJOPROJECT_MANIFEST);
            fs_err::write(
                &mojo_path,
                "[workspace]\nname = \"existing_pixi\"\nchannels = []\nplatforms = []",
            )
            .unwrap();
        }

        let mut env_file = None;
        if config.with_env_file {
            let env_path = project_path.join("environment.yml");
            fs_err::write(
                &env_path,
                "name: env\nchannels: [conda-forge]\ndependencies: [python]",
            )
            .unwrap();
            env_file = Some(env_path);
        }

        let options = InitOptions {
            path: project_path.clone(),
            env_file,
            format: config.format,
            channels: None,
            platforms: vec![],
            scm: None,
            conda_pypi_mapping: None,
        };

        let interface = MockInterface {
            confirm_response: config.confirm_response,
        };
        let result = init(&interface, options).await;

        let pixi_exists = project_path.join(consts::WORKSPACE_MANIFEST).is_file();
        let pyproject_exists = project_path.join(consts::PYPROJECT_MANIFEST).is_file();
        let mojo_exists = project_path.join(consts::MOJOPROJECT_MANIFEST).is_file();

        TestOutcome {
            result,
            project_path,
            pixi_exists,
            pyproject_exists,
            mojo_exists,
            _tmp_dir: tmp_dir,
        }
    }

    #[rstest]
    #[tokio::test]
    async fn test_init_with_env_file_fail_if_pixi_exists(
        #[values(
            None,
            Some(ManifestFormat::Pixi),
            Some(ManifestFormat::Pyproject),
            Some(ManifestFormat::Mojoproject)
        )]
        format: Option<ManifestFormat>,
        #[values(true, false)] pre_existing_pyproject: bool,
        #[values(true, false)] pre_existing_mojo: bool,
        #[values(true, false)] confirm_response: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format,
            with_env_file: true,
            pre_existing_pixi: true,
            pre_existing_pyproject,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        let error = outcome.result.unwrap_err();
        let error_msg = format!("{:?}", error);

        assert!(
            error_msg.contains("pixi.toml already exists"),
            "The command failed, but for the wrong reason. Error texts was: {}",
            error_msg
        )
    }

    #[rstest]
    #[tokio::test]

    async fn test_init_with_env_file_succeeds(
        #[values(
            None,
            Some(ManifestFormat::Pixi),
            Some(ManifestFormat::Pyproject),
            Some(ManifestFormat::Mojoproject)
        )]
        format: Option<ManifestFormat>,
        #[values(true, false)] pre_existing_pyproject: bool,
        #[values(true, false)] pre_existing_mojo: bool,
        #[values(true, false)] confirm_response: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format,
            with_env_file: true,
            pre_existing_pixi: false,
            pre_existing_pyproject,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(outcome.pixi_exists);
    }

    #[rstest]
    #[tokio::test]

    async fn test_prompt_for_extending_pyproject_confirm_yes_not_extended(
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject: true,
            pre_existing_mojo,
            confirm_response: true,
            pyproject_already_extended: false,
        })
        .await;

        // let workspace = outcome.result.unwrap();
        assert!(outcome.result.is_ok());
        assert!(outcome.pyproject_exists);

        // check that the relevant table was added to pyproject file
        let pyproject_path = outcome.project_path.join(consts::PYPROJECT_MANIFEST);
        let content = fs_err::read_to_string(pyproject_path).unwrap();
        assert!(
            content.contains("[tool.pixi.workspace]"),
            "Pyproject.toml should include [tool.pixi.workspace] after extending"
        )
    }

    #[rstest]
    #[tokio::test]

    async fn test_prompt_for_extending_pyproject_confirm_no(
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject: true,
            pre_existing_mojo,
            confirm_response: false,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(outcome.pyproject_exists);
        assert!(outcome.pixi_exists);

        let pyproject_path = outcome.project_path.join(consts::PYPROJECT_MANIFEST);
        let content = fs_err::read_to_string(pyproject_path).unwrap();
        assert!(
            !content.contains("[tool.pixi.workspace]"),
            "Pyproject.toml shouldn't include [tool.pixi.workspace] as extending didn't take place"
        )
    }

    #[rstest]
    #[tokio::test]

    async fn test_prompt_for_extending_pyproject_confirm_yes_already_extended(
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject: true,
            pre_existing_mojo,
            confirm_response: true,
            pyproject_already_extended: true,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(outcome.pyproject_exists);

        let pyproject_path = outcome.project_path.join(consts::PYPROJECT_MANIFEST);
        let content = fs_err::read_to_string(pyproject_path).unwrap();

        let pixi_table_count = content.matches("[tool.pixi.workspace]").count();

        assert_eq!(
            pixi_table_count, 1,
            "The [tool.pixi.workspace] table was duplicated! Early exit failed."
        );
    }

    #[rstest]
    #[tokio::test]
    async fn test_new_pyproject_format(
        #[values(true, false)] pre_existing_mojo: bool,
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pixi: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Pyproject),
            with_env_file: false,
            pre_existing_pixi,
            pre_existing_pyproject: false,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(outcome.pyproject_exists);
        assert_eq!(outcome.pixi_exists, pre_existing_pixi);

        let pyproject_path = outcome.project_path.join(consts::PYPROJECT_MANIFEST);
        let content = fs_err::read_to_string(pyproject_path).unwrap();
        assert!(
            content.contains("[tool.pixi.workspace]"),
            "Pyproject.toml should include [tool.pixi.workspace]"
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_mojo_format_fails_if_mojo_exists(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pixi: bool,
        #[values(true, false)] pre_existing_pyproject: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Mojoproject),
            with_env_file: false,
            pre_existing_pixi,
            pre_existing_pyproject,
            pre_existing_mojo: true,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        let error = outcome.result.unwrap_err();
        let error_msg = format!("{:?}", error);

        assert!(
            error_msg.contains("mojoproject.toml already exists"),
            "The command failed, but for the wrong reason. Error texts was: {}",
            error_msg
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_mojo_format_succeeds(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pixi: bool,
        #[values(true, false)] pre_existing_pyproject: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Mojoproject),
            with_env_file: false,
            pre_existing_pixi,
            pre_existing_pyproject,
            pre_existing_mojo: false,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.pyproject_exists, pre_existing_pyproject);
        assert_eq!(outcome.pixi_exists, pre_existing_pixi);
        assert!(outcome.mojo_exists);
    }

    #[rstest]
    #[tokio::test]
    async fn test_pixi_format_succeeds(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pyproject: bool,
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Pixi),
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.pyproject_exists, pre_existing_pyproject);
        assert_eq!(outcome.mojo_exists, pre_existing_mojo);
        assert!(outcome.pixi_exists);
    }

    #[rstest]
    #[tokio::test]
    async fn test_pixi_format_fails_if_pixi_exists(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_pyproject: bool,
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: Some(ManifestFormat::Pixi),
            with_env_file: false,
            pre_existing_pixi: true,
            pre_existing_pyproject,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        let error = outcome.result.unwrap_err();
        let error_msg = format!("{:?}", error);

        assert!(
            error_msg.contains("pixi.toml already exists"),
            "The command failed, but for the wrong reason. Error texts was: {}",
            error_msg
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_init_default_fails_if_pixi_exists(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: true,
            pre_existing_pyproject: false,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        let error = outcome.result.unwrap_err();
        let error_msg = format!("{:?}", error);

        assert!(
            error_msg.contains("pixi.toml already exists"),
            "The command failed, but for the wrong reason. Error texts was: {}",
            error_msg
        )
    }

    #[rstest]
    #[tokio::test]
    async fn test_default_format_succeeds(
        #[values(true, false)] confirm_response: bool,
        #[values(true, false)] pre_existing_mojo: bool,
    ) {
        let outcome = run_init_scenario(TestConfig {
            format: None,
            with_env_file: false,
            pre_existing_pixi: false,
            pre_existing_pyproject: false,
            pre_existing_mojo,
            confirm_response,
            pyproject_already_extended: false,
        })
        .await;

        assert!(outcome.result.is_ok());
        assert!(!outcome.pyproject_exists);
        assert_eq!(outcome.mojo_exists, pre_existing_mojo);
        assert!(outcome.pixi_exists);
    }
}
