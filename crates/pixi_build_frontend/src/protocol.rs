use std::path::Path;

use crate::{
    backend::BackendSpec, conda_build::CondaBuildProtocol, pixi::PixiProtocol, BackendOverrides,
};

/// A protocol describes how to communicate with a build backend. A build
/// backend is a tool that is invoked to generate certain output.
///
/// The frontend can support multiple backends, and the protocol is used to
/// determine which backend to use for a given source directory and how to
/// communicate with it.
#[derive(Debug)]
pub(crate) enum Protocol {
    /// A pixi project.
    Pixi(PixiProtocol),

    /// A directory containing a `meta.yaml` that can be interpreted by
    /// conda-build.
    CondaBuild(CondaBuildProtocol),
}

impl From<PixiProtocol> for Protocol {
    fn from(value: PixiProtocol) -> Self {
        Self::Pixi(value)
    }
}

impl From<CondaBuildProtocol> for Protocol {
    fn from(value: CondaBuildProtocol) -> Self {
        Self::CondaBuild(value)
    }
}

impl Protocol {
    /// Discovers the protocol for the given source directory.
    pub fn discover(
        source_dir: &Path,
        overrides: BackendOverrides,
    ) -> miette::Result<Option<Self>> {
        if source_dir.is_file() {
            miette::bail!("source directory must be a directory");
        } else if !source_dir.is_dir() {
            miette::bail!("cannot find source directory '{}'", source_dir.display());
        }

        // Try to discover as a pixi project
        if let Some(protocol) = PixiProtocol::discover(source_dir, &overrides)? {
            return Ok(Some(protocol.into()));
        }

        // Try to discover as a conda build project
        if let Some(protocol) = CondaBuildProtocol::discover(source_dir, &overrides)? {
            return Ok(Some(protocol.into()));
        }

        // TODO: Add additional formats later
        Ok(None)
    }

    /// Returns the name of the protocol.
    pub fn name(&self) -> &str {
        match self {
            Self::Pixi(_) => "pixi",
            Protocol::CondaBuild(_) => "conda-build",
        }
    }

    /// Returns a build tool specification for the protocol. This describes how
    /// to acquire the build tool for the specific package.
    pub fn backend_spec(&self) -> &BackendSpec {
        match self {
            Self::Pixi(protocol) => protocol.backend_spec(),
            Self::CondaBuild(protocol) => protocol.backend_spec(),
        }
    }
}
