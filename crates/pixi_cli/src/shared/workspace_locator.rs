use std::sync::Arc;

use itertools::Itertools;
use miette::Report;
use pixi_core::{WorkspaceLocator, workspace::LocatorWarnings};

/// A [`WorkspaceLocator`] for CLI commands that shows the warnings it
/// encounters.
pub(crate) fn workspace_locator() -> WorkspaceLocator {
    WorkspaceLocator::for_cli().with_warning_handler(Arc::new(show_warnings))
}

fn show_warnings(warnings: LocatorWarnings) {
    match warnings {
        LocatorWarnings::Manifest(warnings) => tracing::warn!(
            "Encountered {} warning{} while parsing the manifest:\n{}",
            warnings.len(),
            if warnings.len() == 1 { "" } else { "s" },
            warnings
                .into_iter()
                .map(Report::from)
                .format_with("\n", |w, f| f(&format_args!("{w:?}")))
        ),
        LocatorWarnings::Script(warnings) => {
            for warning in warnings {
                tracing::warn!("{:?}", Report::from(warning));
            }
        }
        LocatorWarnings::ShellManifestIgnored {
            used_manifest,
            shell_manifest,
        } => tracing::warn!(
            "Using local manifest {} rather than {} from environment variable `PIXI_WORKSPACE_MANIFEST`",
            used_manifest.display(),
            shell_manifest.display(),
        ),
    }
}
