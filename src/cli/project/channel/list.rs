use clap::Parser;

use crate::Project;
use fancy_display::FancyDisplay;
use pixi_manifest::FeaturesExt;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// Whether to display the channel's names or urls
    #[clap(long)]
    pub urls: bool,
}

pub fn execute(project: Project, args: Args) -> miette::Result<()> {
    let channel_config = project.channel_config();
    project
        .environments()
        .iter()
        .map(|e| {
            println!(
                "{} {}",
                console::style("Environment:").bold().bright(),
                e.name().fancy_display()
            );
            e.channels()
        })
        .for_each(|c| {
            c.into_iter().for_each(|channel| {
                println!(
                    "- {}",
                    if args.urls {
                        channel.clone().into_base_url(&channel_config).to_string()
                    } else {
                        channel.to_string()
                    }
                );
            })
        });
    Ok(())
}
