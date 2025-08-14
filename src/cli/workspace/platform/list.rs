use pixi_core::Workspace;
use fancy_display::FancyDisplay;
use pixi_manifest::FeaturesExt;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    workspace
        .environments()
        .iter()
        .map(|e| {
            println!(
                "{} {}",
                console::style("Environment:").bold().bright(),
                e.name().fancy_display()
            );
            e.platforms()
        })
        .for_each(|c| {
            c.into_iter().for_each(|platform| {
                println!("- {}", platform.as_str());
            })
        });
    Ok(())
}
