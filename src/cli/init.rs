use crate::{config::get_default_author, consts};
use clap::Parser;
use minijinja::{context, Environment};
use rattler_conda_types::Platform;
use std::{fs, path::PathBuf};

/// Creates a new project
#[derive(Parser, Debug)]
pub struct Args {
    /// Where to place the project (defaults to current path)
    #[arg(default_value = ".")]
    path: PathBuf,
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
channels = ["{{ channel }}"]
platforms = ["{{ platform }}"]

[commands]

[dependencies]
"#;

const GITIGNORE_TEMPLATE: &str = r#"# pixi environments
.pixi
"#;

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let env = Environment::new();
    let dir = args.path;
    let manifest_path = dir.join(consts::PROJECT_MANIFEST);
    let gitignore_path = dir.join(".gitignore");

    // Check if the project file doesnt already exist. We don't want to overwrite it.
    if fs::metadata(&manifest_path).map_or(false, |x| x.is_file()) {
        anyhow::bail!("{} already exists", consts::PROJECT_MANIFEST);
    }

    // Fail silently if it already exists or cannot be created.
    fs::create_dir_all(&dir).ok();

    // Write pixi.toml
    let name = dir.file_name().unwrap().to_string_lossy();
    let version = "0.1.0";
    let author = get_default_author();
    let channel = "conda-forge";
    let platform = Platform::current();

    let rv = env
        .render_named_str(
            consts::PROJECT_MANIFEST,
            PROJECT_TEMPLATE,
            context! {
                name,
                version,
                author,
                channel,
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
