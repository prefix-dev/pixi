use crate::Workspace;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    println!("{}", workspace.name());
    Ok(())
}
