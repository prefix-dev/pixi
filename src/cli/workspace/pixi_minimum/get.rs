use crate::Workspace;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    println!(
        "{}",
        workspace
            .workspace
            .value
            .workspace
            .pixi_minimum
            .unwrap_or(rattler_conda_types::Version::major(0))
    );
    Ok(())
}
