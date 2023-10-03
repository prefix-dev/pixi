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

    /// Require pixi.lock is up-to-date
    #[clap(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Don't check if pixi.lock is up-to-date, install as lockfile states
    #[clap(long, conflicts_with = "locked")]
    pub frozen: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    get_up_to_date_prefix(&project, args.frozen, args.locked).await?;

    // Emit success
    eprintln!(
        "{}Project in {} is ready to use!",
        console::style(console::Emoji("✔ ", "")).green(),
        project.root().display()
    );
    Ok(())
}
