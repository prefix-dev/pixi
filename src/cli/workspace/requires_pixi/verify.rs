use pixi_core::Workspace;
use pixi_manifest::ExplicitManifestError;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    // exit code:
    //   0: success
    //   1: failed to parse manifest
    //   2: failed to parse command line arguments
    //   4: current pixi version is old
    match workspace.verify_current_pixi_meets_requirement() {
        Ok(_) => Ok(()),
        Err(e) => {
            if let ExplicitManifestError::SelfVersionMatchError { .. } = e {
                eprintln!(
                    "Error:   {}{}",
                    console::style(console::Emoji("× ", "")).red(),
                    e
                );
                std::process::exit(4);
            } else {
                Err(e.into())
            }
        }
    }
}
