use pixi_api::Workspace;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    println!("{}", workspace.display_name());
    Ok(())
}
