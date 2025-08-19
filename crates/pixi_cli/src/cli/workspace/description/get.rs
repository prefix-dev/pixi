use pixi_core::Workspace;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    // Print the description if it exists
    if let Some(description) = workspace.workspace.value.workspace.description {
        println!("{}", description);
    }
    Ok(())
}
