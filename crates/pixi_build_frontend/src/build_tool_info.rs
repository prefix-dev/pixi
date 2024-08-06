use miette::{Context, IntoDiagnostic};
use pixi_manifest::Manifest;
use rattler_conda_types::{MatchSpec, ParseMatchSpecError};
use std::path::Path;

use crate::options::{BuildToolSpec, PixiBuildFrontendOptions};

/// Build info to be returned by the build tool
#[derive(Debug)]
pub struct BuildToolInfo {
    /// Build tool spec to use
    pub build_tool: BuildToolSpec,
}

impl BuildToolInfo {
    /// TODO: refactor add different spec types later
    /// Returns the build information to be used in the project
    pub fn from_pixi(
        manifest_path: &Path,
        opts: &PixiBuildFrontendOptions,
    ) -> miette::Result<BuildToolInfo> {
        // Use the global tool if specified
        if let Some(tool) = &opts.override_build_tool {
            return Ok(BuildToolInfo {
                build_tool: tool.clone(),
            });
        }

        // Get the build tool from the manifest
        let manifest = Manifest::from_path(manifest_path).wrap_err("error loading manifest")?;
        Self::from_manifest(&manifest)
            .into_diagnostic()
            .wrap_err("error reading build info from manifest")
    }

    fn from_manifest(_manifest: &Manifest) -> Result<BuildToolInfo, ParseMatchSpecError> {
        // TODO: Get correct information from the manifest
        // For now, just use the python backend
        let match_spec = MatchSpec::from_str(
            "pixi_build_python",
            rattler_conda_types::ParseStrictness::Strict,
        )?;

        Ok(BuildToolInfo {
            build_tool: BuildToolSpec::CondaPackage(match_spec),
        })
    }
}
