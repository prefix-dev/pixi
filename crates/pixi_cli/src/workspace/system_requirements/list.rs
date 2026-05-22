use clap::Parser;
use pixi_core::Workspace;
use pixi_manifest::EnvironmentName;

#[derive(Parser, Debug)]
pub struct Args {
    /// Kept for backwards-compatibility; ignored.
    #[clap(long)]
    pub json: bool,

    /// Kept for backwards-compatibility; ignored.
    #[clap(long, short)]
    pub environment: Option<EnvironmentName>,
}

pub(crate) fn execute(_workspace: &Workspace, _args: Args) -> miette::Result<()> {
    eprintln!(
        "`pixi workspace system-requirements list` is deprecated. Use `pixi workspace platform list` to view per-platform virtual packages instead."
    );
    Ok(())
}
