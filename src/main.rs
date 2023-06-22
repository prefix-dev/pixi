use console::style;
use pixi::cli;
use pixi::report_error;

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
