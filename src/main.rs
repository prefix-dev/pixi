use pixi::cli;
use pixi::unix::PtySession;
use std::process::Command;

#[tokio::main]
pub async fn main() {
    // if let Err(err) = cli::execute().await {
    //     eprintln!("{err:?}");
    //     std::process::exit(1);
    // }

    let mut process = PtySession::new(Command::new("bash")).unwrap();
    process.interact().await.unwrap();
}
