use crate::Project;

pub async fn execute(project: Project) -> miette::Result<()> {
    // Print the description if it exists
    if let Some(description) = project.description() {
        eprintln!("{}", description);
    }
    Ok(())
}
