use std::{
    collections::BTreeMap,
    fmt::{self, Display},
    hash::{Hash, Hasher},
    sync::Arc,
};

use miette::Diagnostic;
use pixi_command_dispatcher::{
    ComputeCtx, EnvironmentRef, HasWorkspaceEnvRegistry, InstalledSourceHints, Key, PtrArc,
    SourceRecordError, WorkspaceEnvRegistry,
    compute_data::HasCacheDirs,
    keys::{ResolveSourcePackageKey, ResolveSourcePackageSpec},
};
use pixi_record::{PinnedBuildSourceSpec, PinnedSourceSpec, UnresolvedPixiRecord, VariantValue};
use pixi_spec::SourceLocationSpec;
use rattler_conda_types::PackageName;
use thiserror::Error;
use tracing::instrument;
use xxhash_rust::xxh3::Xxh3;

use super::cache;

/// Compute-engine [`Key`] that produces the build and host environments
/// of a source record loaded from a pre-v7 lock file.
///
/// Pre-v7 lock files do not store these environments, so the in-memory
/// [`UnresolvedSourceRecord`](pixi_record::UnresolvedSourceRecord)
/// arrives with empty `build_packages` / `host_packages`. This Key
/// dispatches [`ResolveSourcePackageKey`] to recompute them from the
/// build backend (the same path that produces them at lock time for v7
/// records), then picks the variant matching the locked record.
///
/// The recursion through nested source dependencies is handled
/// internally by [`ResolveSourcePackageKey`] via `nested_solve`, so
/// nested source records in the returned vectors come back with their
/// own `build_packages` / `host_packages` already populated.
///
/// `installed_source_hints` is an optimization input that seeds nested
/// solves with the prior resolution; it must be excluded from any
/// persistent cache key derived from this Key, since it does not
/// affect correctness of the result, only the cost of computing it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LegacySourceEnvKey {
    /// The package name from the locked source record.
    pub package: PackageName,

    /// Pinned manifest source from the locked record. Pre-v7 lock files
    /// already pin this; the pin flows through unchanged.
    pub manifest_source: PinnedSourceSpec,

    /// Pinned build source override, if any, from the locked record.
    pub build_source: Option<PinnedBuildSourceSpec>,

    /// Variants from the locked record, used to pick the matching
    /// output among the variants returned by
    /// [`ResolveSourcePackageKey`].
    pub variants: BTreeMap<String, VariantValue>,

    /// Workspace environment context for the resolution.
    pub env_ref: EnvironmentRef,

    /// Pin map flowed through to nested solves so deeper layers see
    /// pins for every package they transitively reference.
    pub preferred_build_source: Arc<BTreeMap<PackageName, PinnedSourceSpec>>,

    /// Optimization-only seed for nested solves. Pointer-identity hashed,
    /// so two calls with the same logical hints but different `Arc`
    /// allocations dedup independently in the in-memory cache.
    pub installed_source_hints: PtrArc<InstalledSourceHints>,
}

/// Build and host environments produced for a single locked source
/// record. The shape mirrors the `build_packages` / `host_packages`
/// fields on `SourceRecord` so callers can write the result onto the
/// record directly.
#[derive(Clone, Debug)]
pub struct LegacySourceEnv {
    pub build_packages: Vec<UnresolvedPixiRecord>,
    pub host_packages: Vec<UnresolvedPixiRecord>,
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum LegacySourceEnvError {
    /// The underlying [`ResolveSourcePackageKey`] failed.
    #[error(transparent)]
    Resolve(#[from] SourceRecordError),

    /// The backend returned outputs but none matched the locked
    /// record's variants. This usually means the source's variant
    /// matrix changed since the lock file was written.
    #[error(
        "backend produced no variant matching the locked record for `{package}` (locked variants: {variants:?})"
    )]
    NoMatchingVariant {
        package: String,
        variants: BTreeMap<String, VariantValue>,
    },
}

impl Display for LegacySourceEnvKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@{} in {}",
            self.package.as_source(),
            self.manifest_source,
            self.env_ref
        )
    }
}

impl LegacySourceEnvKey {
    /// Stable hash for the on-disk cache filename. Resolves
    /// [`EnvironmentRef`] to its underlying [`EnvironmentSpec`] via the
    /// registry so the hash is content-driven (registry ids are
    /// allocation-order-dependent and thus unstable across runs).
    /// Excludes `installed_source_hints` (optimization input only); the
    /// cache directory itself already separates workspaces, so it does
    /// not participate in the hash either.
    fn cache_key_hash(&self, registry: &WorkspaceEnvRegistry) -> u64 {
        let mut hasher = Xxh3::new();
        self.package.hash(&mut hasher);
        self.manifest_source.hash(&mut hasher);
        self.build_source.hash(&mut hasher);
        self.variants.hash(&mut hasher);
        let env_spec = self.env_ref.resolve(registry);
        env_spec.hash(&mut hasher);
        self.preferred_build_source.hash(&mut hasher);
        hasher.finish()
    }
}

impl Key for LegacySourceEnvKey {
    type Value = Result<LegacySourceEnv, LegacySourceEnvError>;

    #[instrument(
        skip_all,
        name = "legacy-source-env",
        fields(
            name = %self.package.as_source(),
            source = %self.manifest_source,
            platform = %self.env_ref.display_platform(),
        )
    )]
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        // Try the on-disk cache first. A miss here (file absent, schema
        // bump, parse error) just falls through to recompute.
        let registry = ctx.global_data().workspace_env_registry().clone();
        let cache_dir = ctx.global_data().cache_dirs().legacy_source_env();
        let cache_hash = self.cache_key_hash(&registry);
        let cache_path = cache::cache_file_path(cache_dir.as_std_path(), cache_hash);
        if let Some(env) = cache::load(&cache_path) {
            tracing::debug!(
                package = %self.package.as_source(),
                source = %self.manifest_source,
                "legacy-source-env cache hit"
            );
            return Ok(env);
        }

        let variants = ctx
            .compute(&ResolveSourcePackageKey::new(ResolveSourcePackageSpec {
                package: self.package.clone(),
                source_location: SourceLocationSpec::from(self.manifest_source.clone()),
                preferred_build_source: self.preferred_build_source.clone(),
                env_ref: self.env_ref.clone(),
                installed_source_hints: self.installed_source_hints.clone(),
            }))
            .await?;

        let matched = variants
            .iter()
            .find(|sr| variants_equivalent(&self.variants, &sr.variants))
            .ok_or_else(|| LegacySourceEnvError::NoMatchingVariant {
                package: self.package.as_source().to_string(),
                variants: self.variants.clone(),
            })?;

        let env = LegacySourceEnv {
            build_packages: matched.build_packages.clone(),
            host_packages: matched.host_packages.clone(),
        };
        cache::store(&cache_path, &env);
        Ok(env)
    }
}

/// Compare two variant maps while ignoring the synthetic
/// `target_platform` key that the backend injects but pre-v7 lock files
/// omit. Mirrors the policy used by
/// `verify_partial_source_record_against_backend` for the
/// locked-vs-backend case; here both sides are
/// [`pixi_record::VariantValue`] because the resolved record we are
/// matching against has already been normalized.
fn variants_equivalent(
    a: &BTreeMap<String, VariantValue>,
    b: &BTreeMap<String, VariantValue>,
) -> bool {
    let is_real = |k: &str| k != "target_platform";
    let a_count = a.keys().filter(|k| is_real(k.as_str())).count();
    let b_count = b.keys().filter(|k| is_real(k.as_str())).count();
    if a_count != b_count {
        return false;
    }
    a.iter()
        .filter(|(k, _)| is_real(k.as_str()))
        .all(|(k, v)| b.get(k) == Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_equivalent_ignores_target_platform() {
        let mut locked = BTreeMap::new();
        let mut produced = BTreeMap::new();
        produced.insert(
            "target_platform".to_string(),
            VariantValue::from("linux-64".to_string()),
        );
        assert!(variants_equivalent(&locked, &produced));

        locked.insert("python".to_string(), VariantValue::from("3.12".to_string()));
        produced.insert("python".to_string(), VariantValue::from("3.12".to_string()));
        assert!(variants_equivalent(&locked, &produced));
    }

    #[test]
    fn variants_equivalent_mismatched_real_key_fails() {
        let mut locked = BTreeMap::new();
        locked.insert("python".to_string(), VariantValue::from("3.11".to_string()));
        let mut produced = BTreeMap::new();
        produced.insert("python".to_string(), VariantValue::from("3.12".to_string()));
        assert!(!variants_equivalent(&locked, &produced));
    }
}
