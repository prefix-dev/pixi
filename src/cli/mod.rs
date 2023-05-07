use clap::Parser;

mod add;
mod init;
mod run;
mod sync;
mod auth;

#[derive(Parser, Debug)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
enum Command {
    Init(init::Args),
    Add(add::Args),
    Sync(sync::Args),
    Run(run::Args),
    Auth(auth::Args),
}

pub async fn execute() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Init(cmd) => init::execute(cmd).await,
        Command::Add(cmd) => add::execute(cmd).await,
        Command::Sync(cmd) => sync::execute(cmd).await,
        Command::Run(cmd) => run::execute(cmd).await,
        Command::Auth(cmd) => auth::execute(cmd).await,
    }
}
