pub mod legacy;
pub mod pypi_metadata;

mod environment;
mod errors;
mod platform;
mod pypi;
mod source_record;

#[cfg(test)]
mod tests;

// Public surface preserved verbatim from the pre-split `mod.rs`. Some
// of these items were `pub` in the previous monolithic file but never
// re-exported up to `lock_file::*`; rustc surfaces those as "unused
// import" warnings now that they live behind module-internal `use`
// statements. Allow them at the re-export site so the public surface
// matches the pre-split module exactly.
#[allow(unused_imports)]
pub use environment::{PypiNoBuildCheck, verify_environment_satisfiability};
#[allow(unused_imports)]
pub use errors::{
    BuildOrHostEnv, EnvironmentUnsat, ExcludeNewerMismatch, IndexesMismatch, LocalMetadataMismatch,
    PlatformUnsat, SolveGroupUnsat, SourceExcludeNewerMismatch, SourceRunDepKind,
    SourceTreeHashMismatch,
};
#[allow(unused_imports)]
pub use platform::{
    CondaPackageIdx, Dependency, PlatformSatisfiabilityResult, PypiPackageIdx,
    VerifiedIndividualEnvironment, VerifySatisfiabilityContext, resolve_dev_dependencies,
    verify_platform_satisfiability, verify_solve_group_satisfiability,
};
#[allow(unused_imports)]
pub(crate) use pypi::{pypi_satisfies_editable, pypi_satisfies_requirement};

// Re-exports for items referenced from the test module via
// `super::super::*` paths. Visibility stays at `pub(super)` so they
// remain accessible to descendant modules (i.e. `tests`) without
// becoming part of the crate's public surface.
#[cfg(test)]
pub(super) use source_record::{
    build_full_source_record_from_output, diff_dep_sequences, variants_equivalent,
    verify_locked_against_backend_specs, verify_locked_run_deps_against_backend,
};
