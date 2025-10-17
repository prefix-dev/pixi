use std::io::Write;

use fancy_display::FancyDisplay;
use pixi_core::Workspace;
use pixi_manifest::FeaturesExt;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    workspace
        .environments()
        .iter()
        .map(|e| {
            let _ = writeln!(
                std::io::stdout(),
                "{} {}",
                console::style("Environment:").bold().bright(),
                e.name().fancy_display()
            )
            .inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            });
            e.platforms()
        })
        .for_each(|c| {
            c.into_iter().for_each(|platform| {
                let _ = writeln!(std::io::stdout(), "- {}", platform.as_str()).inspect_err(|e| {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        std::process::exit(0);
                    }
                });
            })
        });
    Ok(())
}
