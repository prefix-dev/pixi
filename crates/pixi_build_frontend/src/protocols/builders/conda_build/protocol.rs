use std::{path::PathBuf, sync::OnceLock};

use miette::{Context, IntoDiagnostic};
use pixi_build_types::{
    procedures::conda_metadata::{CondaMetadataParams, CondaMetadataResult},
    CondaPackageMetadata,
};
use rattler_conda_types::{ChannelConfig, NoArchType, PackageName, Platform, VersionWithSource};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use sha1::{Digest, Sha1};

use crate::tool::Tool;

#[derive(Debug)]
pub struct Protocol {
    pub(super) _channel_config: ChannelConfig,
    pub(super) tool: Tool,
    pub(super) source_dir: PathBuf,
    pub(super) recipe_dir: PathBuf,
}

impl Protocol {
    /// Returns a unique identifier for the backend.
    pub fn backend_identifier(&self) -> &str {
        "conda-build"
    }

    /// Returns the relative path from the source directory to the recipe.
    pub fn manifests(&self) -> Vec<String> {
        self.recipe_dir
            .strip_prefix(&self.source_dir)
            .unwrap_or(&self.recipe_dir)
            .join("meta.yaml")
            .to_str()
            .map(|s| s.to_string())
            .into_iter()
            .collect()
    }

    // Extract metadata from the recipe.
    pub fn conda_get_metadata(
        &self,
        request: &CondaMetadataParams,
    ) -> miette::Result<CondaMetadataResult> {
        // Construct a new tool that can be used to invoke conda-render instead of the
        // original tool.
        let conda_render_executable = String::from("conda-render");
        let conda_render_executable = if cfg!(windows) {
            format!("{}.exe", conda_render_executable)
        } else {
            conda_render_executable
        };

        let conda_render_tool = self.tool.with_executable(conda_render_executable);

        // TODO: Properly pass channels
        // TODO: Setup --exclusive-config-files

        let channels = request
            .channel_base_urls
            .iter()
            .flatten()
            .flat_map(|url| ["--channel", url.as_str()]);

        let output = conda_render_tool
            .command()
            // .arg("--verbose")
            // This is currently apparently broken in conda-build..
            // .arg("--use-channeldata")
            .arg("--override-channels")
            .args(channels)
            .arg(&self.recipe_dir)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::piped())
            .output()
            .into_diagnostic()
            .context("failed to spawn conda-render executable")?;

        // Try to parse the contents of the output.
        let stdout = String::from_utf8(output.stdout)
            .into_diagnostic()
            .context("failed to convert the output of conda-render to a valid utf-8 string")?;

        // Fail if the process did not exit successfully.
        if !output.status.success() {
            miette::bail!(
                "conda-render returned with a non-zero exit code:\n{}",
                stdout
            );
        }

        // Parse the output of conda-render.
        let rendered_recipes = extract_rendered_recipes(&stdout)?;

        let metadata = CondaMetadataResult {
            packages: rendered_recipes
                .into_iter()
                .map(|(recipe, meta_yaml)| {
                    convert_conda_render_output(recipe).with_context(|| {
                        format!(
                            "failed to extract metadata from conda-render output:\n{}",
                            meta_yaml
                        )
                    })
                })
                .collect::<miette::Result<_>>()?,
            input_globs: None,
        };

        Ok(metadata)
    }
}

/// Given output from `conda-render`, parse it into one or more
/// [`CondaRenderRecipe`]s.
fn extract_rendered_recipes(
    rendered_recipe: &str,
) -> miette::Result<Vec<(CondaRenderRecipe, &str)>> {
    static OUTPUT_REGEX: OnceLock<Regex> = OnceLock::new();
    let output_regex = OUTPUT_REGEX.get_or_init(|| {
        Regex::new(r#"(?sR)Hash contents:\r?\n-{14}\r?\n(.+?)-{10}\r?\nmeta.yaml:\r?\n-{10}\r?\n(.+?)(?:-{14}|$)"#)
            .unwrap()
    });

    let mut iter = output_regex.captures_iter(rendered_recipe).peekable();
    if iter.peek().is_none() {
        miette::bail!(
            "could not find metadata in conda-render output:\n{}",
            rendered_recipe
        )
    }

    iter.map(|captures| {
        let hash = captures.get(1).unwrap().as_str().trim();
        let meta_yaml = captures.get(2).unwrap().as_str().trim();
        serde_yaml::from_str(meta_yaml)
            .map(|recipe| {
                (
                    CondaRenderRecipe {
                        hash_content: hash.to_string().replace("\r\n", "\n"),
                        recipe,
                    },
                    meta_yaml,
                )
            })
            .into_diagnostic()
            .with_context(|| format!("failed to parse the rendered recipe:\n{meta_yaml}"))
    })
    .collect()
}

/// Converts a [`CondaRenderRecipe`] output into a [`CondaPackageMetadata`].
fn convert_conda_render_output(recipe: CondaRenderRecipe) -> miette::Result<CondaPackageMetadata> {
    Ok(CondaPackageMetadata {
        build: recipe.hash(),
        name: recipe.recipe.package.name,
        version: recipe.recipe.package.version,
        build_number: recipe.recipe.build.number.unwrap_or(0),
        subdir: if recipe.recipe.build.noarch.is_none() {
            Platform::current()
        } else {
            Platform::NoArch
        },
        depends: recipe.recipe.requirements.run,
        constraints: recipe.recipe.requirements.run_constrained,
        license: recipe.recipe.about.license,
        license_family: recipe.recipe.about.license_family,
        noarch: recipe.recipe.build.noarch,
    })
}

#[derive(Debug, Deserialize, Serialize)]
struct CondaRenderRecipe {
    hash_content: String,
    recipe: RenderedRecipe,
}

impl CondaRenderRecipe {
    /// Determine the hash of the recipe. This is based on the user specified
    /// hash or the hash computed from the hash content.
    pub fn hash(&self) -> String {
        // TODO: Verify if this logic is actually correct.

        if let Some(hash) = &self.recipe.build.string {
            return hash.clone();
        }

        let mut hasher = Sha1::new();
        hasher.update(self.hash_content.as_bytes());
        let result = hasher.finalize();

        const HASH_LENGTH: usize = 7;

        let res = format!("{:x}", result);
        res[..HASH_LENGTH].to_string()
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct RenderedRecipe {
    package: RenderedPackage,
    build: RenderedBuild,
    requirements: RenderedRequirements,
    about: RenderedAbout,
}

#[derive(Debug, Deserialize, Serialize)]
struct RenderedPackage {
    name: PackageName,
    version: VersionWithSource,
}

#[serde_as]
#[derive(Debug, Deserialize, Serialize)]
struct RenderedBuild {
    #[serde_as(as = "Option<serde_with::PickFirst<(_, serde_with::DisplayFromStr)>>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    string: Option<String>,
    #[serde(default, skip_serializing_if = "NoArchType::is_none")]
    noarch: NoArchType,
}

#[derive(Debug, Deserialize, Serialize)]
struct RenderedRequirements {
    #[serde(default)]
    run: Vec<String>,
    #[serde(default)]
    run_constrained: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RenderedAbout {
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license_family: Option<String>,
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use itertools::Itertools;
    use rstest::*;

    use super::*;

    #[rstest]
    #[case::pinject("conda-render/pinject.txt")]
    #[case::microarch("conda-render/microarch-level.txt")]
    fn test_extract_rendered_recipe(#[case] path: &str) {
        let rendered_recipe = fs_err::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("test-data")
                .join(path),
        )
        .unwrap();
        let rendered_recipe = extract_rendered_recipes(&rendered_recipe)
            .into_iter()
            .flatten()
            .format_with("\n===\n", |(recipe, meta_yaml), formatter| {
                formatter(&format_args!(
                    "{meta_yaml}\n---\n{}",
                    serde_yaml::to_string(&recipe).unwrap()
                ))
            })
            .to_string();
        insta::assert_snapshot!(path, rendered_recipe);
    }
}
