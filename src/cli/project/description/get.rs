use crate::Workspace;

pub async fn execute(project: Workspace) -> miette::Result<()> {
    // Print the description if it exists
    if let Some(description) = project.description() {
        println!("{}", description);
    }
    Ok(())
}
