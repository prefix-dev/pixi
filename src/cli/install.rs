use crate::environment::get_up_to_date_prefix;
use crate::Project;
use clap::Parser;
use std::path::PathBuf;

/// Install all dependencies
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to a projects manifest path. Default is `pixi.toml`.
    ///
    /// The pixi.toml is searched for in the current dir or lower in the directory tree.
    #[arg(long)]
    manifest_path: Option<PathBuf>,
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = match args.manifest_path {
        Some(path) => Project::load(path.as_path())?,
        None => Project::discover()?,
    };

    get_up_to_date_prefix(&project).await?;

    // Emit success
    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );
    Ok(())
}
