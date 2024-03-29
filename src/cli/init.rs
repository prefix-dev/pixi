use crate::config::Config;
use crate::environment::{get_up_to_date_prefix, LockFileUsage};
use crate::project::manifest::python::PyPiPackageName;
use crate::project::manifest::PyPiRequirement;
use crate::utils::conda_environment_file::{CondaEnvDep, CondaEnvFile};
use crate::{config::get_default_author, consts};
use crate::{FeatureName, Project};
use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use minijinja::{context, Environment};
use pyproject_toml::PyProjectToml;
use rattler_conda_types::ParseStrictness::{Lenient, Strict};
use rattler_conda_types::{Channel, MatchSpec, Platform};
use regex::Regex;
use std::collections::HashMap;
use std::io::{Error, ErrorKind, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
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
const PYROJECT_TEMPLATE: &str = r#"
[tool.pixi.project]
name = "{{ name }}"
channels = [{%- if channels %}"{{ channels|join("\", \"") }}"{%- endif %}]
platforms = ["{{ platforms|join("\", \"") }}"]
{%- for env, features in environments|items %}
{%- if loop.first %}

[tool.pixi.environments]
default = {features = [], solve-group = "default"}
{%- endif %}
{{env}} = {features = {{features}}, solve-group = "default"}
{%- endfor %}

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
    let manifest_path = dir.join(consts::PROJECT_MANIFEST);
    let gitignore_path = dir.join(".gitignore");
    let gitattributes_path = dir.join(".gitattributes");
    let config = Config::load_global();

    // Check if the project file doesn't already exist. We don't want to overwrite it.
    if fs::metadata(&manifest_path).map_or(false, |x| x.is_file()) {
        miette::bail!("{} already exists", consts::PROJECT_MANIFEST);
    }

    // Fail silently if it already exists or cannot be created.
    fs::create_dir_all(&dir).ok();

    let version = "0.1.0";
    let author = get_default_author();
    let platforms = if args.platforms.is_empty() {
        vec![Platform::current().to_string()]
    } else {
        args.platforms.clone()
    };

    // If env file load that else use default template only
    if let Some(env_file) = args.env_file {
        let conda_env_file = CondaEnvFile::from_path(&env_file)?;

        let name = match conda_env_file.name() {
            // Default to something to avoid errors
            None => get_name_from_dir(&dir).unwrap_or_else(|_| String::from("new_project")),
            Some(name) => name.to_string(),
        };

        // TODO: Improve this:
        //  - Use .condarc as channel config
        //  - Implement it for `[crate::project::manifest::ProjectManifest]` to do this for other filetypes, e.g. (pyproject.toml, requirements.txt)
        let (conda_deps, pypi_deps, channels) = conda_env_to_manifest(conda_env_file, &config)?;
        let rv = render_project(&env, name, version, &author, channels, &platforms);
        let mut project = Project::from_str(&dir.join("pixi.toml"), &rv)?;
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
        // Default to something to avoid errors
        let name = get_name_from_dir(&dir).unwrap_or_else(|_| String::from("new_project"));

        let channels = if let Some(channels) = args.channels {
            channels
        } else {
            config.default_channels().to_vec()
        };

        // Inject a tool.pixi.project section into an existing pyproject.toml file if there is one without
        if dir.join(consts::PYPROJECT_MANIFEST).is_file() {
            let path = dir.join(consts::PYPROJECT_MANIFEST);
            let file = fs::read_to_string(path.clone()).unwrap();
            if !file.contains("[tool.pixi.project]") {
                let pyproject = PyProjectToml::new(&file).unwrap();
                let environments = environments_from_extras(&pyproject);
                let rv = env
                    .render_named_str(
                        consts::PYPROJECT_MANIFEST,
                        PYROJECT_TEMPLATE,
                        context! {
                            name,
                            channels,
                            platforms,
                            environments,
                        },
                    )
                    .unwrap();
                if let Err(e) = {
                    fs::OpenOptions::new()
                        .append(true)
                        .open(path.clone())
                        .and_then(|mut p| p.write_all(rv.as_bytes()))
                } {
                    tracing::warn!(
                        "Warning, couldn't update '{}' because of: {}",
                        path.to_string_lossy(),
                        e
                    );
                }
            }

        // Create a pixi.toml
        } else {
            let rv = render_project(&env, name, version, &author, channels, &platforms);
            fs::write(&manifest_path, rv).into_diagnostic()?;
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

type PipReq = (PyPiPackageName, PyPiRequirement);
type ParsedDependencies = (Vec<MatchSpec>, Vec<PipReq>, Vec<Arc<Channel>>);

fn conda_env_to_manifest(
    env_info: CondaEnvFile,
    config: &Config,
) -> miette::Result<(Vec<MatchSpec>, Vec<PipReq>, Vec<String>)> {
    let channels = parse_channels(env_info.channels().clone());
    let (conda_deps, pip_deps, mut extra_channels) =
        parse_dependencies(env_info.dependencies().clone())?;

    extra_channels.extend(
        channels
            .into_iter()
            .map(|c| Arc::new(Channel::from_str(c, config.channel_config()).unwrap())),
    );
    let mut channels: Vec<_> = extra_channels
        .into_iter()
        .unique()
        .map(|c| {
            if c.base_url()
                .as_str()
                .starts_with(config.channel_config().channel_alias.as_str())
            {
                c.name().to_string()
            } else {
                c.base_url().to_string()
            }
        })
        .collect();
    if channels.is_empty() {
        channels = config.default_channels();
    }

    Ok((conda_deps, pip_deps, channels))
}

fn parse_dependencies(deps: Vec<CondaEnvDep>) -> miette::Result<ParsedDependencies> {
    let mut conda_deps = vec![];
    let mut pip_deps = vec![];
    let mut picked_up_channels = vec![];
    for dep in deps {
        match dep {
            CondaEnvDep::Conda(d) => {
                let match_spec = MatchSpec::from_str(&d, Lenient).into_diagnostic()?;
                if let Some(channel) = match_spec.clone().channel {
                    picked_up_channels.push(channel);
                }
                conda_deps.push(match_spec);
            }
            CondaEnvDep::Pip { pip } => pip_deps.extend(
                pip.into_iter()
                    .map(|mut dep| {
                        let re = Regex::new(r"/([^/]+)\.git").unwrap();
                        if let Some(caps) = re.captures(dep.as_str()) {
                            let name= caps.get(1).unwrap().as_str().to_string();
                            tracing::warn!("The dependency '{}' is a git repository, as that is not available in pixi we'll try to install it as a package with the name: {}", dep, name);
                            dep = name;
                        }
                        let req = pep508_rs::Requirement::from_str(&dep).into_diagnostic()?;
                        let name = PyPiPackageName::from_normalized(req.name.clone());
                        let requirement = PyPiRequirement::from(req);
                        Ok((name, requirement))
                    })
                    .collect::<miette::Result<Vec<_>>>()?,
            ),
        }
    }

    if !pip_deps.is_empty()
        && !conda_deps.iter().any(|spec| {
            spec.name
                .as_ref()
                .filter(|name| name.as_normalized() == "pip")
                .is_some()
        })
    {
        conda_deps.push(MatchSpec::from_str("pip", Strict).into_diagnostic()?);
    }

    Ok((conda_deps, pip_deps, picked_up_channels))
}

fn parse_channels(channels: Vec<String>) -> Vec<String> {
    let mut new_channels = vec![];
    for channel in channels {
        if channel == "defaults" {
            // https://docs.anaconda.com/free/working-with-conda/reference/default-repositories/#active-default-channels
            new_channels.push("main".to_string());
            new_channels.push("r".to_string());
            new_channels.push("msys2".to_string());
        } else {
            let channel = channel.trim();
            if !channel.is_empty() {
                new_channels.push(channel.to_string());
            }
        }
    }
    new_channels
}

/// Builds a list of pixi environments from pyproject groups of extra dependencies:
///  - one environment is created per group of extra, with the same name as the group of extra
///  - each environment includes the feature of the same name as the group of extra
///  - it will also include other features inferred from any self references to other groups of extras
pub fn environments_from_extras(pyproject: &PyProjectToml) -> HashMap<String, Vec<String>> {
    let mut environments = HashMap::new();
    if let Some(Some(extras)) = &pyproject.project.as_ref().map(|p| &p.optional_dependencies) {
        let pname = &pyproject
            .project
            .as_ref()
            .map(|p| pep508_rs::PackageName::new(p.name.clone()).unwrap());
        for (extra, reqs) in extras {
            let mut features = vec![extra.to_string()];
            // Add any references to other groups of extra dependencies
            for req in reqs.iter() {
                if pname.as_ref().is_some_and(|n| n == &req.name) {
                    for extra in &req.extras {
                        features.push(extra.to_string())
                    }
                }
            }
            environments.insert(extra.clone(), features);
        }
    }
    environments
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init::get_dir;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn test_parse_conda_env_file() {
        let example_conda_env_file = r#"
        name: pixi_example_project
        channels:
          - conda-forge
          - https://custom-server.com/channel
        dependencies:
          - python
          - pytorch::torchvision
          - conda-forge::pytest
          - wheel=0.31.1
          - sel(linux): blabla
          - foo >=1.2.3.*  # only valid when parsing in lenient mode
          - pip:
            - requests
            - git+https://git@github.com/fsschneider/DeepOBS.git@develop#egg=deepobs
            - torch==1.8.1
        "#;

        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(example_conda_env_file.as_bytes()).unwrap();
        let (_file, path) = f.into_parts();

        let conda_env_file_data = CondaEnvFile::from_path(&path).unwrap();

        assert_eq!(conda_env_file_data.name(), Some("pixi_example_project"));
        assert_eq!(
            conda_env_file_data.channels(),
            &vec![
                "conda-forge".to_string(),
                "https://custom-server.com/channel".to_string()
            ]
        );

        let config = Config::default();
        let (conda_deps, pip_deps, channels) =
            conda_env_to_manifest(conda_env_file_data, &config).unwrap();

        assert_eq!(
            channels,
            vec![
                "pytorch".to_string(),
                "conda-forge".to_string(),
                "https://custom-server.com/channel/".to_string()
            ]
        );

        println!("{conda_deps:?}");
        assert_eq!(
            conda_deps,
            vec![
                MatchSpec::from_str("python", Strict).unwrap(),
                MatchSpec::from_str("pytorch::torchvision", Strict).unwrap(),
                MatchSpec::from_str("conda-forge::pytest", Strict).unwrap(),
                MatchSpec::from_str("wheel=0.31.1", Strict).unwrap(),
                MatchSpec::from_str("foo >=1.2.3", Strict).unwrap(),
                MatchSpec::from_str("pip", Strict).unwrap(),
            ]
        );

        assert_eq!(
            pip_deps,
            vec![
                (
                    PyPiPackageName::from_str("requests").unwrap(),
                    PyPiRequirement::default()
                ),
                (
                    PyPiPackageName::from_str("deepobs").unwrap(),
                    PyPiRequirement::default()
                ),
                (
                    PyPiPackageName::from_str("torch").unwrap(),
                    PyPiRequirement::Version {
                        version: "==1.8.1".parse().unwrap(),
                        index: None,
                        extras: vec![],
                    }
                ),
            ]
        );
    }

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

    #[test]
    fn test_import_from_env_yamls() {
        let test_files_path = Path::new(&env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("environment_yamls");

        let entries = match fs::read_dir(test_files_path) {
            Ok(entries) => entries,
            Err(e) => panic!("Failed to read directory: {}", e),
        };

        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.expect("Failed to read directory entry");
            if entry.path().is_file() {
                paths.push(entry.path());
            }
        }

        for path in paths {
            let env_info = CondaEnvFile::from_path(&path).unwrap();
            // Try `cargo insta test` to run all at once
            let snapshot_name = format!(
                "test_import_from_env_yaml.{}",
                path.file_name().unwrap().to_string_lossy()
            );

            insta::assert_debug_snapshot!(
                snapshot_name,
                (
                    parse_dependencies(env_info.dependencies().clone()).unwrap(),
                    parse_channels(env_info.channels().clone()),
                    env_info.name()
                )
            );
        }
    }
    #[test]
    fn test_parse_conda_env_file_with_explicit_pip_dep() {
        let example_conda_env_file = r#"
        name: pixi_example_project
        channels:
          - conda-forge
        dependencies:
          - pip==24.0
          - pip:
            - requests
        "#;

        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path();
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(example_conda_env_file.as_bytes()).unwrap();

        let conda_env_file_data = CondaEnvFile::from_path(path).unwrap();
        let (conda_deps, pip_deps, _) =
            parse_dependencies(conda_env_file_data.dependencies().clone()).unwrap();

        assert_eq!(
            conda_deps,
            vec![MatchSpec::from_str("pip==24.0", Strict).unwrap(),]
        );

        assert_eq!(
            pip_deps,
            vec![(
                PyPiPackageName::from_str("requests").unwrap(),
                PyPiRequirement::default()
            ),]
        );
    }
}
