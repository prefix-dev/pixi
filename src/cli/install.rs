use crate::environment::get_up_to_date_prefix;
use crate::Project;
use clap::Parser;
use std::path::PathBuf;

/// Install the dependencies of the project
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to a pixi project
    #[arg(long)]
    project_path: Option<PathBuf>,
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::discover(args.project_path)?;
    get_up_to_date_prefix(&project).await?;
    // Emit success
    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );
    Ok(())
}
