use crate::Workspace;
use pixi_manifest::ExplicitManifestError;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    // exit code:
    //   0: success
    //   1: failed to parse manifest
    //   2: failed to parse command line arguments
    //   3: current pixi version is old
    match workspace.pixi_minimum_version() {
        Ok(_) => Ok(()),
        Err(ExplicitManifestError::SelfVersionMatchError { .. }) => std::process::exit(3),
        Err(e) => Err(e.into()),
    }
}
