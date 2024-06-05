use itertools::Itertools;

use crate::Project;

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
