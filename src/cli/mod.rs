use clap::Parser;

mod add;
mod init;
mod run;
mod sync;

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
}

pub async fn execute() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Init(cmd) => init::execute(cmd).await,
        Command::Add(cmd) => add::execute(cmd).await,
        Command::Sync(cmd) => sync::execute(cmd).await,
        Command::Run(cmd) => run::execute(cmd).await,
    }
}
