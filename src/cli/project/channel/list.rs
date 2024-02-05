use crate::consts::ENV_STYLE;
use crate::Project;
use clap::Parser;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// Whether to display the channel's names or urls
    #[clap(long)]
    pub urls: bool,
}

pub fn execute(project: Project, args: Args) -> miette::Result<()> {
    project
        .environments()
        .iter()
        .map(|e| {
            println!(
                "{} {}",
                console::style("Environment:").bold().bright(),
                ENV_STYLE.apply_to(e.name())
            );
            e.channels()
        })
        .for_each(|c| {
            c.into_iter().for_each(|channel| {
                println!(
                    "- {}",
                    if args.urls {
                        channel.base_url().as_str()
                    } else {
                        channel.name()
                    }
                );
            })
        });
    Ok(())
}
