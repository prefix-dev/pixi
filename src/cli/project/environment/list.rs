use crate::fancy_display::FancyDisplay;
use crate::Project;
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
