#[tokio::main]
pub async fn main() -> miette::Result<()> {
    pixi::cli::execute().await
}
