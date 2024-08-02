use crate::Project;
use fancy_display::FancyDisplay;
use itertools::Itertools;

pub async fn execute(project: Project) -> miette::Result<()> {
    println!(
        "{}",
        project
            .environments()
            .iter()
            .format_with("\n", |e, f| f(&format_args!(
                "- {}",
                e.name().fancy_display()
            )))
    );

    Ok(())
}
