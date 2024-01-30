#[tokio::main]
pub async fn main() {
    if let Err(err) = pixi::cli::execute().await {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}
