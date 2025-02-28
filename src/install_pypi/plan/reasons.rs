#[derive(Debug)]
pub enum InstallReason {
    /// Reinstall a package from the local cache, will link from the cache
    ReinstallCached,
    /// Reinstall a package that we have determined to be stale, will be taken from the registry
    ReinstallStaleLocal,
    /// Reinstall a package that is missing from the local cache, but is available in the registry
    ReinstallMissing,
    /// Install a package from the local cache, will link from the cache
    InstallCached,
    /// Install a package that we have determined to be stale, will be taken from the registry
    InstallStaleLocal,
    /// Install a package that is missing from the local cache, but is available in the registry
    InstallMissing,
}

/// This trait can be used to generalize over the different reason why a specific installation source was chosen
/// So we can differentiate between re-installing and installing a package, this is all a bit verbose
/// but can be quite useful for debugging and logging
pub(crate) trait OperationToReason {
    /// This package is available in the local cache
    fn cached(&self) -> InstallReason;
    /// This package is determined to be stale
    fn stale(&self) -> InstallReason;
    /// This package is missing from the local cache
    fn missing(&self) -> InstallReason;
}

/// Use this struct to get the correct install reason
pub(crate) struct Install;
impl OperationToReason for Install {
    fn cached(&self) -> InstallReason {
        InstallReason::InstallCached
    }

    fn stale(&self) -> InstallReason {
        InstallReason::InstallStaleLocal
    }

    fn missing(&self) -> InstallReason {
        InstallReason::InstallMissing
    }
}

/// Use this struct to get the correct reinstall reason
pub(crate) struct Reinstall;
impl OperationToReason for Reinstall {
    fn cached(&self) -> InstallReason {
        InstallReason::ReinstallCached
    }

    fn stale(&self) -> InstallReason {
        InstallReason::ReinstallStaleLocal
    }

    fn missing(&self) -> InstallReason {
        InstallReason::ReinstallMissing
    }
}
