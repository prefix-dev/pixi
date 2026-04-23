use pixi_core::WorkspaceLocatorError;

/// exit code:
///   0: success
///   1: failed to parse manifest
///   2: failed to parse command line arguments
///   4: current pixi version is old
pub fn execute(result: Result<(), WorkspaceLocatorError>) -> miette::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(WorkspaceLocatorError::PixiVersionMismatch(e)) => {
            eprintln!(
                "Error:   {}{}",
                console::style(console::Emoji("× ", "")).red(),
                e
            );

            #[cfg(feature = "self_update")]
            {
                eprintln!();
                eprintln!("Try running:\n  pixi self-update");
            }

            #[cfg(not(feature = "self_update"))]
            {
                eprintln!();
                eprintln!(
                    "Please update pixi using your system package manager or reinstall it.\n\
             See: https://pixi.sh/latest/installation/"
                );
            }

            std::process::exit(4);
        }
        Err(e) => Err(e.into()),
    }
}
