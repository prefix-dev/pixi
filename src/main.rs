use console::style;

mod cli;
mod command;
mod config;
mod consts;
mod environment;
mod prefix;
mod progress;
mod project;
mod repodata;
mod report_error;
mod virtual_packages;

pub use project::Project;

#[tokio::main]
pub async fn main() {
    if let Err(err) = cli::execute().await {
        match err.downcast::<report_error::ReportError>() {
            Ok(report) => report.eprint(),
            Err(err) => {
                eprintln!("{}: {:?}", style("error").bold().red(), err);
            }
        }
        std::process::exit(1);
    }
}
