use crate::environment::get_up_to_date_prefix;
use crate::Project;
use clap::Parser;
use std::path::PathBuf;

/// Install all dependencies
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    get_up_to_date_prefix(&project).await?;

    // Emit success
    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );
    Ok(())
}
