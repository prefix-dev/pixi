use clap::Parser;
use miette::IntoDiagnostic;

use crate::Project;
use fancy_display::FancyDisplay;
use pixi_manifest::FeaturesExt;

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// Whether to display the channel's names or urls
    #[clap(long)]
    pub urls: bool,
}

pub(crate) fn execute(project: Project, args: Args) -> miette::Result<()> {
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
        .try_for_each(|c| -> Result<(), rattler_conda_types::ParseChannelError> {
            c.into_iter().try_for_each(
                |channel| -> Result<(), rattler_conda_types::ParseChannelError> {
                    println!(
                        "- {}",
                        if args.urls {
                            match channel.clone().into_base_url(&channel_config) {
                                Ok(url) => url.to_string(),
                                Err(e) => return Err(e),
                            }
                        } else {
                            channel.to_string()
                        }
                    );
                    Ok(())
                },
            )
        })
        .into_diagnostic()?;
    Ok(())
}
