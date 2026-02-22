#[derive(Debug, Clone)]
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
    InstallStaleCached,
    /// Install a package that is missing from the local cache, but is available in the registry
    InstallMissing,
}
