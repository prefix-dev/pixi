use pixi::cli;

#[tokio::main]
pub async fn main() {
    if let Err(err) = cli::execute().await {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}
