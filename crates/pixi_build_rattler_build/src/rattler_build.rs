use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use miette::IntoDiagnostic;
use pixi_build_backend::source::Source;
use pixi_build_types::SourcePackageSpec;
use rattler_build::console_utils::LoggingOutputHandler;

use crate::config::RattlerBuildBackendConfig;

pub struct RattlerBuildBackend {
    pub(crate) logging_output_handler: LoggingOutputHandler,
    pub(crate) source_dir: PathBuf,
    /// In case of rattler-build, manifest is the raw recipe
    /// We need to apply later the selectors to get the final recipe
    pub(crate) recipe_source: Source,
    pub(crate) manifest_root: PathBuf,
    pub(crate) cache_dir: Option<PathBuf>,
    pub(crate) config: RattlerBuildBackendConfig,
    /// Workspace dependencies from the project model
    pub(crate) workspace_dependencies: HashMap<String, SourcePackageSpec>,
}

impl RattlerBuildBackend {
    /// Returns a new instance of [`RattlerBuildBackend`] by reading the
    /// manifest at the given path.
    pub fn new(
        source_dir: Option<PathBuf>,
        manifest_path: &Path,
        logging_output_handler: LoggingOutputHandler,
        cache_dir: Option<PathBuf>,
        config: RattlerBuildBackendConfig,
    ) -> miette::Result<Self> {
        // Locate the recipe
        let manifest_file_name = manifest_path.file_name().and_then(OsStr::to_str);
        let (recipe_path, source_dir) = match manifest_file_name {
            Some("recipe.yaml") | Some("recipe.yml") => {
                let source_dir = source_dir.unwrap_or_else(|| {
                    manifest_path
                        .parent()
                        .expect("file always has parent")
                        .to_path_buf()
                });
                (manifest_path.to_path_buf(), source_dir)
            }
            _ => {
                // The manifest is not a recipe, so we need to find the recipe.yaml file.
                let source_dir = source_dir.unwrap_or_else(|| {
                    manifest_path
                        .parent()
                        .unwrap_or(manifest_path)
                        .to_path_buf()
                });
                let recipe_path = if let Some(recipe_path_local) = config.recipe.clone() {
                    if !recipe_path_local.is_absolute() {
                        source_dir.join(recipe_path_local)
                    } else {
                        recipe_path_local
                    }
                } else {
                    manifest_path.parent().and_then(|manifest_dir| {
                        [
                            "recipe.yaml",
                            "recipe.yml",
                            "recipe/recipe.yaml",
                            "recipe/recipe.yml",
                        ]
                        .into_iter()
                        .find_map(|relative_path| {
                            let recipe_path = manifest_dir.join(relative_path);
                            recipe_path.is_file().then_some(recipe_path)
                        })
                    })
                    .ok_or_else(|| miette::miette!("Could not find a recipe.yaml in the source directory to use as the recipe manifest."))?
                };

                (recipe_path, source_dir)
            }
        };

        // Load the manifest from the source directory
        let manifest_root = manifest_path
            .parent()
            .expect("manifest must have a root")
            .to_path_buf();
        let recipe_source =
            Source::from_rooted_path(&manifest_root, recipe_path).into_diagnostic()?;

        Ok(Self {
            logging_output_handler,
            source_dir,
            recipe_source,
            manifest_root,
            cache_dir,
            config,
            workspace_dependencies: HashMap::new(),
        })
    }
}
