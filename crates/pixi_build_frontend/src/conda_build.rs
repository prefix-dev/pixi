use std::path::{Path, PathBuf};

use rattler_conda_types::{MatchSpec, ParseStrictness::Strict};

use crate::{
    backend::{BackendSpec, IsolatedBackendSpec},
    BackendOverrides,
};

#[derive(Debug, Clone)]
pub struct CondaBuildProtocol {
    _source_dir: PathBuf,
    _recipe_dir: PathBuf,
    backend_spec: BackendSpec,
}

impl CondaBuildProtocol {
    /// Discovers the protocol for the given source directory.
    pub fn discover(
        source_dir: &Path,
        overrides: &BackendOverrides,
    ) -> miette::Result<Option<Self>> {
        let recipe_dir = source_dir.join("recipe");
        let protocol = if source_dir.join("meta.yaml").is_file() {
            Self::new(source_dir, source_dir)
        } else if recipe_dir.join("meta.yaml").is_file() {
            Self::new(source_dir, &recipe_dir)
        } else {
            return Ok(None);
        };

        Ok(Some(protocol.with_backend_overrides(overrides.clone())))
    }

    /// Constructs a new instance from a manifest.
    pub fn new(source_dir: &Path, recipe_dir: &Path) -> Self {
        let backend_spec =
            IsolatedBackendSpec::from_specs(vec![
                MatchSpec::from_str("conda-build", Strict).unwrap()
            ])
            .into();

        Self {
            _source_dir: source_dir.to_path_buf(),
            _recipe_dir: recipe_dir.to_path_buf(),
            backend_spec,
        }
    }

    /// Overrides the build tool information with the given overrides.
    pub fn with_backend_overrides(self, overrides: BackendOverrides) -> Self {
        Self {
            backend_spec: overrides.into_spec().unwrap_or(self.backend_spec),
            ..self
        }
    }

    /// Information about the backend tool to install.
    pub fn backend_spec(&self) -> &BackendSpec {
        &self.backend_spec
    }
}
