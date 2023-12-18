use crate::Project;
use clap::Parser;

/// List the channels in the project file.
#[derive(Parser, Debug, Default)]
pub struct Args {
    /// Whether to display the channel's names or urls
    #[clap(long)]
    pub urls: bool,
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    project.channels().iter().for_each(|channel| {
        if args.urls {
            // Print the channel's url
            eprintln!("{}", channel.base_url());
        } else {
            // Print the channel's name and fallback to the url if it doesn't have one
            let name = channel
                .name
                .as_deref()
                .unwrap_or(channel.base_url().as_str());
            eprintln!("{}", name);
        }
    });

    Ok(())
}
