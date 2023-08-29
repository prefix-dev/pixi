use pixi::cli;
use pixi::unix::PtySession;
use std::process::Command;

#[tokio::main]
pub async fn main() {
    if let Err(err) = cli::execute().await {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
    // println!("Starting zsh");
    // let mut process = PtySession::new(Command::new("zsh")).unwrap();
    // process.interact().await.unwrap();
}
