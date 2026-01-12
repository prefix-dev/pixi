use miette::{IntoDiagnostic, WrapErr};
use rattler_build::metadata::Output;
use std::future::Future;
use std::io::BufWriter;
use std::path::PathBuf;

/// A helper struct that owns a temporary file containing a rendered recipe.
/// If `finish` is not called, the temporary file will stay on disk for
/// debugging purposes.
pub struct TemporaryRenderedRecipe {
    file: PathBuf,
}

impl TemporaryRenderedRecipe {
    pub fn from_output(output: &Output) -> miette::Result<Self> {
        // Ensure that the output directory exists
        std::fs::create_dir_all(&output.build_configuration.directories.output_dir)
            .into_diagnostic()
            .context("failed to create output directory")?;

        let (recipe_file, recipe_path) = tempfile::Builder::new()
            .prefix(".rendered-recipe")
            .suffix(".yaml")
            .tempfile_in(&output.build_configuration.directories.output_dir)
            .into_diagnostic()
            .context("failed to create temporary file for recipe")?
            .into_parts();

        // Write the recipe back to a file
        serde_yaml::to_writer(BufWriter::new(recipe_file), &output.recipe)
            .into_diagnostic()
            .context("failed to write recipe to temporary file")?;

        Ok(Self {
            file: recipe_path.keep().unwrap(),
        })
    }

    pub async fn within_context_async<
        R,
        Fut: Future<Output = miette::Result<R>>,
        F: FnOnce() -> Fut,
    >(
        self,
        operation: F,
    ) -> miette::Result<R> {
        let result = operation().await?;
        std::fs::remove_file(self.file)
            .into_diagnostic()
            .context("failed to remove temporary recipe file")?;
        Ok(result)
    }
}
