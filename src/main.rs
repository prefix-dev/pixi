use console::style;

mod cli;
mod config;
mod consts;
mod environment;
mod prefix;
mod progress;
mod project;
mod repodata;
mod virtual_packages;
mod script;

pub use project::Project;

#[tokio::main]
pub async fn main() {
    if let Err(err) = cli::execute().await {
        eprintln!("{}: {:?}", style("error").bold().red(), err);
        std::process::exit(1);
    }
}
