use crate::Workspace;

pub async fn execute(project: Workspace) -> miette::Result<()> {
    println!("{}", project.name());
    Ok(())
}
