//! Compute-engine [`Key`](pixi_compute_engine::Key) implementations for the
//! pixi-environment solve path. [`SolvePixiEnvironmentKey`] is the
//! orchestrator; [`SourceMetadataKey`] and [`SolveCondaKey`] are the leaf
//! work units it composes. Source-record assembly is inlined into the
//! orchestrator rather than promoted to its own Key, so the graph stays flat.

pub mod resolve_source_package;
pub(crate) mod resolve_source_record;
pub mod solve_conda;
pub mod solve_pixi_environment;
pub mod source_build;
pub mod source_metadata;

pub use resolve_source_package::{ResolveSourcePackageKey, ResolveSourcePackageSpec};
pub use solve_conda::{SolveCondaKey, SolveCondaKeyError, SolveCondaSpec};
pub use solve_pixi_environment::{SolvePixiEnvironmentKey, SolvePixiEnvironmentSpec};
pub use source_build::{ArtifactCache, SourceBuildKey, SourceBuildSpec, WorkspaceCache};
pub use source_metadata::{SourceMetadata, SourceMetadataKey, SourceMetadataSpec, SourceOutputs};
