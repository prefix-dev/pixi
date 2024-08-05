use crate::Project;
use fancy_display::FancyDisplay;
use pixi_manifest::FeaturesExt;

pub async fn execute(project: Project) -> miette::Result<()> {
    project
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
