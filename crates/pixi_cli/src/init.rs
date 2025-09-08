use std::{
    cmp::PartialEq,
    collections::HashMap,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::{Parser, ValueEnum};
use miette::{Context, IntoDiagnostic};
use minijinja::{Environment, context};
use pixi_config::{Config, get_default_author, pixi_home};
use pixi_consts::consts;
use pixi_manifest::{FeatureName, pyproject::PyProjectManifest};
use pixi_utils::conda_environment_file::CondaEnvFile;
use rattler_conda_types::{NamedChannelOrUrl, Platform};
use same_file::is_same_file;
use tokio::fs::OpenOptions;
use url::Url;
use uv_normalize::PackageName;

use pixi_core::workspace::WorkspaceMut;

#[derive(Parser, Debug, Clone, PartialEq, ValueEnum)]
pub enum ManifestFormat {
    Pixi,
    Pyproject,
    Mojoproject,
}

/// Creates a new workspace
///
/// This command is used to create a new workspace.
/// It prepares a manifest and some helpers for the user to start working.
///
/// As pixi can both work with `pixi.toml` and `pyproject.toml` files, the user can choose which one to use with `--format`.
///
/// You can import an existing conda environment file with the `--import` flag.
#[derive(Parser, Debug)]
pub struct Args {
    /// Where to place the workspace (defaults to current path)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Channel to use in the workspace.
    #[arg(
        short,
        long = "channel",
        value_name = "CHANNEL",
        conflicts_with = "ENVIRONMENT_FILE"
    )]
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Platforms that the workspace supports.
    #[arg(short, long = "platform", id = "PLATFORM")]
    pub platforms: Vec<String>,

    /// Environment.yml file to bootstrap the workspace.
    #[arg(short = 'i', long = "import", id = "ENVIRONMENT_FILE")]
    pub env_file: Option<PathBuf>,

    /// The manifest format to create.
    #[arg(long, conflicts_with_all = ["ENVIRONMENT_FILE", "pyproject_toml"], ignore_case = true)]
    pub format: Option<ManifestFormat>,

    /// Create a pyproject.toml manifest instead of a pixi.toml manifest
    // BREAK (0.27.0): Remove this option from the cli in favor of the `format` option.
    #[arg(long, conflicts_with_all = ["ENVIRONMENT_FILE", "format"], alias = "pyproject", hide = true)]
    pub pyproject_toml: bool,

    /// Source Control Management used for this workspace
    #[arg(short = 's', long = "scm", ignore_case = true)]
    pub scm: Option<GitAttributes>,
}

/// The pixi.toml template
///
/// This uses a template just to simplify the flexibility of emitting it.
const WORKSPACE_TEMPLATE: &str = r#"[workspace]
{%- if author %}
authors = ["{{ author[0] }} <{{ author[1] }}>"]
{%- endif %}
channels = {{ channels }}
name = "{{ name }}"
platforms = {{ platforms }}
version = "{{ version }}"

{%- if index_url or extra_index_urls %}

[pypi-options]
{% if index_url %}index-url = "{{ index_url }}"{% endif %}
{% if extra_index_urls %}extra-index-urls = {{ extra_index_urls }}{% endif %}
{%- endif %}

{%- if s3 %}
{%- for key in s3 %}

[workspace.s3-options.{{ key }}]
{%- if s3[key]["endpoint-url"] %}
endpoint-url = "{{ s3[key]["endpoint-url"] }}"
{%- endif %}
{%- if s3[key].region %}
{%- endif %}
{%- if s3[key].region %}
region = "{{ s3[key].region }}"
{%- endif %}
{%- if s3[key]["force-path-style"] is not none %}
force-path-style = {{ s3[key]["force-path-style"] }}
{%- endif %}

{%- endfor %}
{%- endif %}

[tasks]

[dependencies]

{%- if env_vars %}

[activation]
env = { {{ env_vars }} }
{%- endif %}

"#;

/// The pyproject.toml template
///
/// This is injected into an existing pyproject.toml
const PYROJECT_TEMPLATE_EXISTING: &str = r#"
[tool.pixi.workspace]
{%- if pixi_name %}
name = "{{ name }}"
{%- endif %}
channels = {{ channels }}
platforms = {{ platforms }}

[tool.pixi.pypi-dependencies]
{{ name }} = { path = ".", editable = true }
{%- for env, features in environments|items %}
{%- if loop.first %}

[tool.pixi.environments]
default = { solve-group = "default" }
{%- endif %}
{{env}} = { features = {{ features }}, solve-group = "default" }
{%- endfor %}

{%- if s3 %}
{%- for key in s3 %}

[tool.pixi.workspace.s3-options.{{ key }}]
{%- if s3[key]["endpoint-url"] %}
endpoint-url = "{{ s3[key]["endpoint-url"] }}"
{%- endif %}
{%- if s3[key].region %}
{%- endif %}
{%- if s3[key].region %}
region = "{{ s3[key].region }}"
{%- endif %}
{%- if s3[key]["force-path-style"] is not none %}
force-path-style = {{ s3[key]["force-path-style"] }}
{%- endif %}

{%- endfor %}
{%- endif %}

[tool.pixi.tasks]

"#;

/// The pyproject.toml template
///
/// This is used to create a pyproject.toml from scratch
const NEW_PYROJECT_TEMPLATE: &str = r#"[project]
{%- if author %}
authors = [{name = "{{ author[0] }}", email = "{{ author[1] }}"}]
{%- endif %}
dependencies = []
name = "{{ name }}"
requires-python = ">= 3.11"
version = "{{ version }}"

[build-system]
build-backend = "hatchling.build"
requires = ["hatchling"]

[tool.pixi.workspace]
channels = {{ channels }}
platforms = {{ platforms }}


{%- if index_url or extra_index_urls %}

[tool.pixi.pypi-options]
{% if index_url %}index-url = "{{ index_url }}"{% endif %}
{% if extra_index_urls %}extra-index-urls = {{ extra_index_urls }}{% endif %}
{%- endif %}

{%- if s3 %}
{%- for key in s3 %}

[tool.pixi.workspace.s3-options.{{ key }}]
{%- if s3[key]["endpoint-url"] %}
endpoint-url = "{{ s3[key]["endpoint-url"] }}"
{%- endif %}
{%- if s3[key].region %}
{%- endif %}
{%- if s3[key].region %}
region = "{{ s3[key].region }}"
{%- endif %}
{%- if s3[key]["force-path-style"] is not none %}
force-path-style = {{ s3[key]["force-path-style"] }}
{%- endif %}

{%- endfor %}
{%- endif %}

[tool.pixi.pypi-dependencies]
{{ pypi_package_name }} = { path = ".", editable = true }

[tool.pixi.tasks]

"#;

const GITIGNORE_TEMPLATE: &str = r#"
# pixi environments
.pixi/*
!.pixi/config.toml
"#;

#[derive(Parser, Debug, Clone, PartialEq, ValueEnum)]
pub enum GitAttributes {
    Github,
    Gitlab,
    Codeberg,
}

impl GitAttributes {
    fn template(&self) -> &'static str {
        match self {
            GitAttributes::Github | GitAttributes::Codeberg => {
                r#"# SCM syntax highlighting & preventing 3-way merges
pixi.lock merge=binary linguist-language=YAML linguist-generated=true
"#
            }
            GitAttributes::Gitlab => {
                r#"# GitLab syntax highlighting & preventing 3-way merges
pixi.lock merge=binary gitlab-language=yaml gitlab-generated=true
"#
            }
        }
    }
}

fn is_init_dir_equal_to_pixi_home_parent(init_dir: &Path) -> bool {
    pixi_home()
        .as_ref()
        .and_then(|home_dir| home_dir.parent())
        .and_then(|parent| is_same_file(parent, init_dir).ok())
        .unwrap_or(false)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let env = Environment::new();
    // Fail silently if the directory already exists or cannot be created.
    fs_err::create_dir_all(&args.path).ok();
    let dir = args.path.canonicalize().into_diagnostic()?;
    let pixi_manifest_path = dir.join(consts::WORKSPACE_MANIFEST);
    let pyproject_manifest_path = dir.join(consts::PYPROJECT_MANIFEST);
    let mojoproject_manifest_path = dir.join(consts::MOJOPROJECT_MANIFEST);
    let gitignore_path = dir.join(".gitignore");
    let gitattributes_path = dir.join(".gitattributes");
    let config = Config::load_global();

    if is_init_dir_equal_to_pixi_home_parent(&dir) {
        let help_msg = format!(
            "Please follow the getting started guide at https://pixi.sh/v{}/init_getting_started/ or run the following command to create a new workspace in a subdirectory:\n\n  {}\n",
            consts::PIXI_VERSION,
            console::style("pixi init my_workspace").bold(),
        );
        miette::bail!(
            help = help_msg,
            "initialization without a name in the parent directory of your PIXI_HOME is not allowed.",
        );
    }

    // Deprecation warning for the `pyproject` option
    if args.pyproject_toml {
        eprintln!(
            "{}The '{}' option is deprecated and will be removed in the future.\nUse '{}' instead.",
            console::style(console::Emoji("⚠️ ", "")).yellow(),
            console::style("--pyproject").bold().red(),
            console::style("--format pyproject").bold().green(),
        );
    }

    let default_name = get_name_from_dir(&dir).unwrap_or_else(|_| String::from("new_workspace"));
    let version = "0.1.0";
    let author = get_default_author();
    let platforms = if args.platforms.is_empty() {
        vec![Platform::current().to_string()]
    } else {
        args.platforms.clone()
    };

    // Create a 'pixi.toml' manifest and populate it by importing a conda
    // environment file
    if let Some(env_file_path) = args.env_file {
        // Check if the 'pixi.toml' file doesn't already exist. We don't want to
        // overwrite it.
        if pixi_manifest_path.is_file() {
            miette::bail!("{} already exists", consts::WORKSPACE_MANIFEST);
        }

        let env_file = CondaEnvFile::from_path(&env_file_path)?;
        let name = env_file
            .name()
            .unwrap_or(default_name.clone().as_str())
            .to_string();

        let env_vars = env_file.variables();
        // TODO: Improve this:
        //  - Use .condarc as channel config
        let (conda_deps, pypi_deps, channels) = env_file.to_manifest(&config)?;
        let rendered_workspace_template = render_workspace(
            &env,
            name,
            version,
            author.as_ref(),
            channels,
            &platforms,
            None,
            &vec![],
            config.s3_options,
            Some(&env_vars),
        );
        let mut workspace =
            WorkspaceMut::from_template(pixi_manifest_path, rendered_workspace_template)?;
        workspace.add_specs(
            conda_deps,
            pypi_deps,
            &[] as &[Platform],
            &FeatureName::default(),
        )?;
        let workspace = workspace.save().await.into_diagnostic()?;

        eprintln!(
            "{}Created {}",
            console::style(console::Emoji("✔ ", "")).green(),
            // Canonicalize the path to make it more readable, but if it fails just use the path as
            // is.
            workspace.workspace.provenance.path.display()
        );
    } else {
        let channels = if let Some(channels) = args.channels {
            channels
        } else {
            config.default_channels().to_vec()
        };

        let index_url = config.pypi_config.index_url;
        let extra_index_urls = config.pypi_config.extra_index_urls;

        // Dialog with user to create a 'pyproject.toml' or 'pixi.toml' manifest
        // If nothing is defined but there is a `pyproject.toml` file, ask the user.
        let pyproject = if !pixi_manifest_path.is_file()
            && args.format.is_none()
            && !args.pyproject_toml
            && pyproject_manifest_path.is_file()
        {
            eprintln!(
                "\nA '{}' file already exists.\n",
                console::style(consts::PYPROJECT_MANIFEST).bold()
            );

            dialoguer::Confirm::new()
                .with_prompt(format!(
                    "Do you want to extend it with the '{}' configuration?",
                    console::style("[tool.pixi]").bold().green()
                ))
                .default(false)
                .show_default(true)
                .interact()
                .into_diagnostic()?
        } else {
            args.format == Some(ManifestFormat::Pyproject) || args.pyproject_toml
        };

        // Inject a tool.pixi.workspace section into an existing pyproject.toml file if
        // there is one without '[tool.pixi.workspace]'
        if pyproject && pyproject_manifest_path.is_file() {
            let pyproject = PyProjectManifest::from_path(&pyproject_manifest_path)?;

            // Early exit if 'pyproject.toml' already contains a '[tool.pixi.workspace]' table
            if pyproject.has_pixi_table() {
                eprintln!(
                    "{}Nothing to do here: 'pyproject.toml' already contains a '[tool.pixi.workspace]' section.",
                    console::style(console::Emoji("🤔 ", "")).blue(),
                );
                return Ok(());
            }

            let (name, pixi_name) = match pyproject.name() {
                Some(name) => (name, false),
                None => (default_name.as_str(), true),
            };
            let environments = pyproject.environments_from_extras().into_diagnostic()?;
            let rv = env
                .render_named_str(
                    consts::PYPROJECT_MANIFEST,
                    PYROJECT_TEMPLATE_EXISTING,
                    context! {
                        name,
                        pixi_name,
                        channels,
                        platforms,
                        environments,
                        s3 => relevant_s3_options(config.s3_options, channels),
                    },
                )
                .expect("should be able to render the template");
            if let Err(e) = {
                fs::OpenOptions::new()
                    .append(true)
                    .open(pyproject_manifest_path.clone())
                    .and_then(|mut p| p.write_all(rv.as_bytes()))
            } {
                tracing::warn!(
                    "Warning, couldn't update '{}' because of: {}",
                    pyproject_manifest_path.to_string_lossy(),
                    e
                );
            } else {
                // Inform about the addition of the package itself as an editable dependency of
                // the workspace
                eprintln!(
                    "{}Added package '{}' as an editable dependency.",
                    console::style(console::Emoji("✔ ", "")).green(),
                    name
                );
                // Inform about the addition of environments from optional dependencies
                // or dependency groups (if any)
                if !environments.is_empty() {
                    let envs: Vec<&str> = environments.keys().map(AsRef::as_ref).collect();
                    eprintln!(
                        "{}Added environment{} '{}' from optional dependencies or dependency groups.",
                        console::style(console::Emoji("✔ ", "")).green(),
                        if envs.len() > 1 { "s" } else { "" },
                        envs.join("', '")
                    )
                }
            }

            // Create a 'pyproject.toml' manifest
        } else if pyproject {
            // Python package names cannot contain '-', so we replace them with '_'
            let pypi_package_name = PackageName::from_str(&default_name)
                .map(|name| name.as_dist_info_name().to_string())
                .unwrap_or_else(|_| default_name.clone());

            let rv = env
                .render_named_str(
                    consts::PYPROJECT_MANIFEST,
                    NEW_PYROJECT_TEMPLATE,
                    context! {
                        name => default_name,
                        pypi_package_name,
                        version,
                        author,
                        channels,
                        platforms,
                        index_url => index_url.as_ref(),
                        extra_index_urls => &extra_index_urls,
                        s3 => relevant_s3_options(config.s3_options, channels),
                    },
                )
                .expect("should be able to render the template");
            save_manifest_file(&pyproject_manifest_path, rv)?;

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
                    return Err(e).into_diagnostic().wrap_err_with(|| {
                        format!("Could not create file {}.", init_file.display())
                    });
                }
            };

        // Create a 'pixi.toml' manifest
        } else {
            let path = if args.format == Some(ManifestFormat::Mojoproject) {
                mojoproject_manifest_path
            } else {
                pixi_manifest_path
            };

            // Check if the manifest file doesn't already exist. We don't want to
            // overwrite it.
            if path.is_file() {
                miette::bail!("{} already exists", consts::WORKSPACE_MANIFEST);
            }

            let rv = render_workspace(
                &env,
                default_name,
                version,
                author.as_ref(),
                channels,
                &platforms,
                index_url.as_ref(),
                &extra_index_urls,
                config.s3_options,
                None,
            );
            save_manifest_file(&path, rv)?;
        };
    }

    // create a .gitignore if one is missing
    if let Err(e) = create_or_append_file(&gitignore_path, GITIGNORE_TEMPLATE.trim_start()) {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            gitignore_path.to_string_lossy(),
            e
        );
    }

    let git_attributes = args.scm.unwrap_or(GitAttributes::Github);

    // create a .gitattributes if one is missing
    if let Err(e) = create_or_append_file(&gitattributes_path, git_attributes.template()) {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            gitattributes_path.to_string_lossy(),
            e
        );
    }

    Ok(())
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
    s3_options: HashMap<String, pixi_config::S3Options>,
    env_vars: Option<&HashMap<String, String>>,
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
            env_vars.iter().map(|(k, v)| format!("{} = \"{}\"", k, v)).collect::<Vec<String>>().join(", ")
        } else {String::new()}},
    };

    env.render_named_str(consts::WORKSPACE_MANIFEST, WORKSPACE_TEMPLATE, ctx)
        .expect("should be able to render the template")
}

fn relevant_s3_options(
    s3_options: HashMap<String, pixi_config::S3Options>,
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
        .into_iter()
        .filter(|(key, _)| s3_buckets.contains(key))
        .collect()
}

/// Save the rendered template to a file, and print a message to the user.
fn save_manifest_file(path: &Path, content: String) -> miette::Result<()> {
    fs_err::write(path, content).into_diagnostic()?;
    eprintln!(
        "{}Created {}",
        console::style(console::Emoji("✔ ", "")).green(),
        // Canonicalize the path to make it more readable, but if it fails just use the path as is.
        dunce::canonicalize(path)
            .unwrap_or(path.to_path_buf())
            .display()
    );
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

#[cfg(test)]
mod tests {
    use std::{io::Read, path::Path};

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

    #[test]
    fn test_multiple_format_values() {
        let test_cases = vec![
            ("pixi", ManifestFormat::Pixi),
            ("PiXi", ManifestFormat::Pixi),
            ("PIXI", ManifestFormat::Pixi),
            ("pyproject", ManifestFormat::Pyproject),
            ("PyPrOjEcT", ManifestFormat::Pyproject),
            ("PYPROJECT", ManifestFormat::Pyproject),
        ];

        for (input, expected) in test_cases {
            let args = Args::try_parse_from(["init", "--format", input]).unwrap();
            assert_eq!(args.format, Some(expected));
        }
    }

    #[test]
    fn test_multiple_scm_values() {
        let test_cases = vec![
            ("github", GitAttributes::Github),
            ("GiThUb", GitAttributes::Github),
            ("GITHUB", GitAttributes::Github),
            ("Github", GitAttributes::Github),
            ("gitlab", GitAttributes::Gitlab),
            ("GiTlAb", GitAttributes::Gitlab),
            ("GITLAB", GitAttributes::Gitlab),
            ("codeberg", GitAttributes::Codeberg),
            ("CoDeBeRg", GitAttributes::Codeberg),
            ("CODEBERG", GitAttributes::Codeberg),
        ];

        for (input, expected) in test_cases {
            let args = Args::try_parse_from(["init", "--scm", input]).unwrap();
            assert_eq!(args.scm, Some(expected));
        }
    }

    #[test]
    fn test_invalid_scm_values() {
        let invalid_values = vec!["invalid", "", "git", "bitbucket", "mercurial", "svn"];

        for value in invalid_values {
            let result = Args::try_parse_from(["init", "--scm", value]);
            assert!(
                result.is_err(),
                "Expected error for invalid SCM value '{}', but got success",
                value
            );
        }
    }
}
