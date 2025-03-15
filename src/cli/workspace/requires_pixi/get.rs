use crate::Workspace;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    println!(
        "{}",
        workspace
            .workspace
            .value
            .workspace
            .requires_pixi
            .unwrap_or(rattler_conda_types::Version::major(0))
    );
    Ok(())
}
