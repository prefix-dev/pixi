use crate::{config::get_default_author, consts};
use anyhow::anyhow;
use clap::Parser;
use minijinja::{context, Environment};
use rattler_conda_types::Platform;
use std::io::{Error, ErrorKind};
use std::{fs, path::PathBuf};

/// Creates a new project
#[derive(Parser, Debug)]
pub struct Args {
    /// Where to place the project (defaults to current path)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Channels to use in the project.
    #[arg(short, long = "channel", id = "channel")]
    pub channels: Vec<String>,
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
channels = ["{{ channels|join("\", \"") }}"]
platforms = ["{{ platform }}"]

[commands]

[dependencies]
"#;

const GITIGNORE_TEMPLATE: &str = r#"# pixi environments
.pixi
"#;

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let env = Environment::new();
    let dir = get_dir(args.path)?;
    let manifest_path = dir.join(consts::PROJECT_MANIFEST);
    let gitignore_path = dir.join(".gitignore");

    // Check if the project file doesnt already exist. We don't want to overwrite it.
    if fs::metadata(&manifest_path).map_or(false, |x| x.is_file()) {
        anyhow::bail!("{} already exists", consts::PROJECT_MANIFEST);
    }

    // Fail silently if it already exists or cannot be created.
    fs::create_dir_all(&dir).ok();

    // Write pixi.toml
    let name = dir
        .file_name()
        .ok_or_else(|| {
            anyhow!(
                "Cannot get file or directory name from the path: {}",
                dir.to_string_lossy()
            )
        })?
        .to_string_lossy();
    let version = "0.1.0";
    let author = get_default_author();
    let channels = if args.channels.is_empty() {
        vec![String::from("conda-forge")]
    } else {
        args.channels
    };
    let platform = Platform::current();

    let rv = env
        .render_named_str(
            consts::PROJECT_MANIFEST,
            PROJECT_TEMPLATE,
            context! {
                name,
                version,
                author,
                channels,
                platform
            },
        )
        .unwrap();
    fs::write(&manifest_path, rv)?;

    // create a .gitignore if one is missing
    if !gitignore_path.is_file() {
        let rv = env.render_named_str("gitignore.txt", GITIGNORE_TEMPLATE, ())?;
        fs::write(&gitignore_path, rv)?;
    }

    // Emit success
    eprintln!(
        "{}Initialized project in {}",
        console::style(console::Emoji("✔ ", "")).green(),
        dir.display()
    );

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
    use crate::cli::init::get_dir;
    use std::path::PathBuf;

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
            PathBuf::from(std::env::current_dir().unwrap().canonicalize().unwrap())
        );
    }

    #[test]
    fn test_get_name_panic() {
        match get_dir(PathBuf::from("invalid/path")) {
            Ok(_) => panic!("Expected error, but got OK"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        }
    }
}
