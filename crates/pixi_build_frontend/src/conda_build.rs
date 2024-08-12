use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{MatchSpec, NoArchType, ParseStrictness::Strict, VersionWithSource};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{
    tool::{IsolatedToolSpec, Tool, ToolSpec},
    Metadata,
};

#[derive(Debug, Clone)]
pub struct CondaBuildProtocol {
    _source_dir: PathBuf,
    recipe_dir: PathBuf,
    backend_spec: ToolSpec,
}

impl CondaBuildProtocol {
    /// Discovers the protocol for the given source directory.
    pub fn discover(source_dir: &Path) -> miette::Result<Option<Self>> {
        let recipe_dir = source_dir.join("recipe");
        let protocol = if source_dir.join("meta.yaml").is_file() {
            Self::new(source_dir, source_dir)
        } else if recipe_dir.join("meta.yaml").is_file() {
            Self::new(source_dir, &recipe_dir)
        } else {
            return Ok(None);
        };

        Ok(Some(protocol))
    }

    /// Constructs a new instance from a manifest.
    pub fn new(source_dir: &Path, recipe_dir: &Path) -> Self {
        let backend_spec =
            IsolatedToolSpec::from_specs(vec![MatchSpec::from_str("conda-build", Strict).unwrap()])
                .into();

        Self {
            _source_dir: source_dir.to_path_buf(),
            recipe_dir: recipe_dir.to_path_buf(),
            backend_spec,
        }
    }

    /// Information about the backend tool to install.
    pub fn backend_tool(&self) -> ToolSpec {
        self.backend_spec.clone()
    }

    /// Extract metadata from the recipe.
    pub fn get_metadata(&self, backend: &Tool) -> miette::Result<Metadata> {
        // Construct a new tool that can be used to invoke conda-render instead of the
        // original tool.
        let conda_render_executable = backend.executable().with_file_name("conda-render");
        let conda_render_executable = if let Some(ext) = backend.executable().extension() {
            conda_render_executable.with_extension(ext)
        } else {
            conda_render_executable
        };
        let conda_render_tool = backend.with_executable(conda_render_executable);

        // TODO: Properly pass channels
        // TODO: Setup --exclusive-config-files

        let output = conda_render_tool
            .command()
            // .arg("--verbose")
            // This is currently apparently broken in conda-build..
            // .arg("--use-channeldata")
            .args(&["--override-channels", "--channel", "conda-forge"])
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

        panic!("{:#?}", rendered_recipes);
    }
}

/// Given output from `conda-render`, parse the rendered recipe.
fn extract_rendered_recipes(rendered_recipe: &str) -> miette::Result<Vec<CondaRenderRecipe>> {
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
            .map(|recipe| CondaRenderRecipe {
                hash_content: hash.to_string(),
                recipe,
            })
            .into_diagnostic()
            .with_context(|| format!("failed to parse the rendered recipe:\n{meta_yaml}"))
    })
    .collect()
}

#[derive(Debug, Deserialize, Serialize)]
struct CondaRenderRecipe {
    hash_content: String,
    recipe: RenderedRecipe,
}

#[derive(Debug, Deserialize, Serialize)]
struct RenderedRecipe {
    package: RenderedPackage,
    build: RenderedBuild,
    requirements: RenderedRequirements,
}

#[derive(Debug, Deserialize, Serialize)]
struct RenderedPackage {
    name: String,
    version: VersionWithSource,
}

#[derive(Debug, Deserialize, Serialize)]
struct RenderedBuild {
    #[serde(skip_serializing_if = "Option::is_none")]
    number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    string: Option<String>,
    #[serde(default, skip_serializing_if = "NoArchType::is_none")]
    noarch: NoArchType,
}

#[derive(Debug, Deserialize, Serialize)]
struct RenderedRequirements {
    run: Vec<String>,
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
    use rstest::*;

    use super::*;

    #[rstest]
    #[case::pinject("conda-render/pinject.txt")]
    #[case::microarch("conda-render/microarch-level.txt")]
    fn test_extract_rendered_recipe(#[case] path: &str) {
        let rendered_recipe = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("test-data")
                .join(path),
        )
        .unwrap();
        let rendered_recipe = extract_rendered_recipes(&rendered_recipe).unwrap();
        insta::assert_yaml_snapshot!(&rendered_recipe);
    }
}
