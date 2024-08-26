use clap::Parser;

/// Lists all packages previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
pub struct Args {}

#[derive(Debug)]
struct InstalledPackageInfo {}

pub async fn execute(_args: Args) -> miette::Result<()> {
    todo!()
}
