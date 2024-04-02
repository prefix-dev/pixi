use crate::config::Config;
use crate::environment::{get_up_to_date_prefix, LockFileUsage};
use crate::utils::conda_environment_file::CondaEnvFile;
use crate::{config::get_default_author, consts};
use crate::{FeatureName, Project};
use clap::Parser;
use indexmap::IndexMap;
use miette::IntoDiagnostic;
use minijinja::{context, Environment};
use pyproject_toml::PyProjectToml;
use rattler_conda_types::Platform;
use std::io::{Error, ErrorKind, Write};
use std::path::Path;
use std::{fs, path::PathBuf};

/// Creates a new project
#[derive(Parser, Debug)]
pub struct Args {
    /// Where to place the project (defaults to current path)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Channels to use in the project.
    #[arg(short, long = "channel", id = "channel", conflicts_with = "env_file")]
    pub channels: Option<Vec<String>>,

    /// Platforms that the project supports.
    #[arg(short, long = "platform", id = "platform")]
    pub platforms: Vec<String>,

    /// Environment.yml file to bootstrap the project.
    #[arg(short = 'i', long = "import")]
    pub env_file: Option<PathBuf>,
}

/// The pixi.toml template
///
/// This uses a template just to simplify the flexibility of emitting it.
const PROJECT_TEMPLATE: &str = r#"[project]
name = "{{ name }}"
version = "{{ version }}"
description = "Add a short description here"
{%- if author %}
authors = ["{{ author[0] }} <{{ author[1] }}>"]
{%- endif %}
channels = [{%- if channels %}"{{ channels|join("\", \"") }}"{%- endif %}]
platforms = ["{{ platforms|join("\", \"") }}"]

[tasks]

[dependencies]

"#;

/// The pyproject.toml template
const PYROJECT_TEMPLATE: &str = r#"
[tool.pixi.project]
name = "{{ name }}"
channels = [{%- if channels %}"{{ channels|join("\", \"") }}"{%- endif %}]
platforms = ["{{ platforms|join("\", \"") }}"]

[tool.pixi.pypi-dependencies]
{{ name }} = { path = ".", editable = true }

"#;

const GITIGNORE_TEMPLATE: &str = r#"# pixi environments
.pixi

"#;

const GITATTRIBUTES_TEMPLATE: &str = r#"# GitHub syntax highlighting
pixi.lock linguist-language=YAML

"#;

pub async fn execute(args: Args) -> miette::Result<()> {
    let env = Environment::new();
    let dir = get_dir(args.path).into_diagnostic()?;
    let pixi_manifest_path = dir.join(consts::PROJECT_MANIFEST);
    let pyproject_manifest_path = dir.join(consts::PYPROJECT_MANIFEST);
    let gitignore_path = dir.join(".gitignore");
    let gitattributes_path = dir.join(".gitattributes");
    let config = Config::load_global();

    // Fail silently if the directory already exists or cannot be created.
    fs::create_dir_all(&dir).ok();

    let default_name = get_name_from_dir(&dir).unwrap_or_else(|_| String::from("new_project"));
    let version = "0.1.0";
    let author = get_default_author();
    let platforms = if args.platforms.is_empty() {
        vec![Platform::current().to_string()]
    } else {
        args.platforms.clone()
    };

    // Create a 'pixi.toml' manifest and populate it by importing a conda environment file
    if let Some(env_file_path) = args.env_file {
        // Check if the 'pixi.toml' file doesn't already exist. We don't want to overwrite it.
        if pixi_manifest_path.is_file() {
            miette::bail!("{} already exists", consts::PROJECT_MANIFEST);
        }

        let env_file = CondaEnvFile::from_path(&env_file_path)?;
        let name = env_file.name().unwrap_or(default_name.as_str()).to_string();

        // TODO: Improve this:
        //  - Use .condarc as channel config
        //  - Implement it for `[crate::project::manifest::ProjectManifest]` to do this for other filetypes, e.g. (pyproject.toml, requirements.txt)
        let (conda_deps, pypi_deps, channels) = env_file.to_manifest(&config)?;
        let rv = render_project(&env, name, version, &author, channels, &platforms);
        let mut project = Project::from_str(&pixi_manifest_path, &rv)?;
        for spec in conda_deps {
            for platform in platforms.iter() {
                // TODO: fix serialization of channels in rattler_conda_types::MatchSpec
                project.manifest.add_dependency(
                    &spec,
                    crate::SpecType::Run,
                    Some(platform.parse().into_diagnostic()?),
                    &FeatureName::default(),
                )?;
            }
        }
        for spec in pypi_deps {
            for platform in platforms.iter() {
                project.manifest.add_pypi_dependency(
                    &spec.0,
                    &spec.1,
                    Some(platform.parse().into_diagnostic()?),
                )?;
            }
        }
        project.save()?;

        get_up_to_date_prefix(
            &project.default_environment(),
            LockFileUsage::Update,
            false,
            IndexMap::default(),
        )
        .await?;
    } else {
        let channels = if let Some(channels) = args.channels {
            channels
        } else {
            config.default_channels().to_vec()
        };

        // Inject a tool.pixi.project section into an existing pyproject.toml file if there is one without '[tool.pixi.project]'
        if pyproject_manifest_path.is_file() {
            let file = fs::read_to_string(pyproject_manifest_path.clone()).unwrap();
            if !file.contains("[tool.pixi.project]") {
                // Get name from the pyproject [project] table
                let name = match PyProjectToml::new(file.as_str()) {
                    Ok(pyproject) => pyproject
                        .project
                        .map(|p| p.name)
                        .expect("'name' should be defined in the [project] table"),
                    Err(e) => miette::bail!("Failed to parse 'pyproject.toml'. Error is {}", e),
                };
                let rv = env
                    .render_named_str(
                        consts::PYPROJECT_MANIFEST,
                        PYROJECT_TEMPLATE,
                        context! {
                            name,
                            channels,
                            platforms
                        },
                    )
                    .unwrap();
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
                }
            }

        // Create a 'pixi.toml' manifest
        } else {
            // Check if the 'pixi.toml' file doesn't already exist. We don't want to overwrite it.
            if pixi_manifest_path.is_file() {
                miette::bail!("{} already exists", consts::PROJECT_MANIFEST);
            }
            let rv = render_project(&env, default_name, version, &author, channels, &platforms);
            fs::write(&pixi_manifest_path, rv).into_diagnostic()?;
        };
    }

    // create a .gitignore if one is missing
    if let Err(e) = create_or_append_file(&gitignore_path, GITIGNORE_TEMPLATE) {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            gitignore_path.to_string_lossy(),
            e
        );
    }

    // create a .gitattributes if one is missing
    if let Err(e) = create_or_append_file(&gitattributes_path, GITATTRIBUTES_TEMPLATE) {
        tracing::warn!(
            "Warning, couldn't update '{}' because of: {}",
            gitattributes_path.to_string_lossy(),
            e
        );
    }

    // Emit success
    eprintln!(
        "{}Initialized project in {}",
        console::style(console::Emoji("✔ ", "")).green(),
        dir.display()
    );

    Ok(())
}

fn render_project(
    env: &Environment<'_>,
    name: String,
    version: &str,
    author: &Option<(String, String)>,
    channels: Vec<String>,
    platforms: &Vec<String>,
) -> String {
    env.render_named_str(
        consts::PROJECT_MANIFEST,
        PROJECT_TEMPLATE,
        context! {
            name,
            version,
            author,
            channels,
            platforms
        },
    )
    .unwrap()
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
    let file = fs::read_to_string(path).unwrap_or_default();

    if !file.contains(template) {
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?
            .write_all(template.as_bytes())?;
    }
    Ok(())
}

fn get_dir(path: PathBuf) -> Result<PathBuf, Error> {
    if path.components().count() == 1 {
        Ok(std::env::current_dir().unwrap_or_default().join(path))
    } else {
        path.canonicalize().map_err(|e| match e.kind() {
            ErrorKind::NotFound => Error::new(
                ErrorKind::NotFound,
                format!(
                    "Cannot find '{}' please make sure the folder is reachable",
                    path.to_string_lossy()
                ),
            ),
            _ => Error::new(
                ErrorKind::InvalidInput,
                "Cannot canonicalize the given path",
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init::get_dir;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn test_get_name() {
        assert_eq!(
            get_dir(PathBuf::from(".")).unwrap(),
            std::env::current_dir().unwrap()
        );
        assert_eq!(
            get_dir(PathBuf::from("test_folder")).unwrap(),
            std::env::current_dir().unwrap().join("test_folder")
        );
        assert_eq!(
            get_dir(std::env::current_dir().unwrap()).unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
    }

    #[test]
    fn test_get_name_panic() {
        match get_dir(PathBuf::from("invalid/path")) {
            Ok(_) => panic!("Expected error, but got OK"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        }
    }

    #[test]
    fn test_create_or_append_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file.txt");
        let template = "Test Template";

        fn read_file_content(path: &Path) -> String {
            let mut file = std::fs::File::open(path).unwrap();
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
}
