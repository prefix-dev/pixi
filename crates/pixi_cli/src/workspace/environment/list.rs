use std::io::Write;

use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_consts::consts;
use pixi_core::Workspace;
use pixi_manifest::HasFeaturesIter;

pub async fn execute(workspace: Workspace) -> miette::Result<()> {
    writeln!(
        std::io::stdout(),
        "Environments:\n{}",
        workspace
            .environments()
            .iter()
            .format_with("\n", |e, f| f(&format_args!(
                "- {}: \n    features: {}{}",
                e.name().fancy_display(),
                e.features().map(|f| f.name.fancy_display()).format(", "),
                if let Some(solve_group) = e.solve_group() {
                    format!(
                        "\n    solve_group: {}",
                        consts::SOLVE_GROUP_STYLE.apply_to(solve_group.name())
                    )
                } else {
                    "".to_string()
                }
            )))
    )
    .inspect_err(|e| {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
    })
    .into_diagnostic()?;

    Ok(())
}
