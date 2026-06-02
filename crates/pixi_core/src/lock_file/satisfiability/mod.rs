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
