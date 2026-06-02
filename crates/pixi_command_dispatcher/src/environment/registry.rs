use std::{collections::HashMap, sync::Arc};

use parking_lot::RwLock;
use pixi_compute_engine::DataStore;
use rattler_conda_types::Platform;

use super::{EnvironmentSpec, WorkspaceEnvId, WorkspaceEnvRef};

/// Registry mapping [`WorkspaceEnvId`] to an immutable
/// [`Arc<EnvironmentSpec>`]. Ids are dense from zero and serve as direct
/// indices into the backing vec. Repeated allocations for the same
/// `(name, platform, spec)` return the existing ref instead of minting
/// another id.
///
/// Stored in the engine's [`DataStore`] as `Arc<WorkspaceEnvRegistry>`.
/// The dispatcher also keeps its own `Arc` clone so callers can reach
/// the registry without a DataStore round-trip.
///
/// Projections resolve a ref via a plain [`get`](Self::get) lookup,
/// not `ctx.compute`, so the read is **not** tracked by the compute
/// engine's dep graph. That's safe because the registry is
/// stable: a given id maps to one immutable `Arc<EnvironmentSpec>` for
/// its entire lifetime. When a spec's content would logically change,
/// the caller allocates again; equal allocation requests reuse the old
/// id, changed requests create a new id.
pub struct WorkspaceEnvRegistry {
    inner: RwLock<WorkspaceEnvRegistryInner>,
}

#[derive(Default)]
struct WorkspaceEnvRegistryInner {
    entries: Vec<Arc<EnvironmentSpec>>,
    refs_by_key: HashMap<WorkspaceEnvRegistryKey, WorkspaceEnvRef>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct WorkspaceEnvRegistryKey {
    name: String,
    platform: Platform,
    spec: EnvironmentSpec,
}

impl WorkspaceEnvRegistry {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(WorkspaceEnvRegistryInner::default()),
        }
    }

    /// Allocate a ref wrapping `(id, name, platform)`, reusing an
    /// existing ref when the same `(name, platform, spec)` has already
    /// been allocated. The write lock protects both the dedup index and
    /// fresh id allocation, so concurrent allocations of equal requests
    /// can't race into distinct ids.
    ///
    /// **Do not call from inside a `Key::compute` body.** Every call
    /// can allocate a new id for new input; doing so from a compute body
    /// could change the compute's dep set between re-runs and defeat
    /// memoization. Internal recursion should use
    /// [`EnvironmentRef::Ephemeral`](super::EnvironmentRef::Ephemeral)
    /// or receive an already-allocated ref from its caller.
    pub fn allocate(
        &self,
        name: String,
        platform: Platform,
        spec: EnvironmentSpec,
    ) -> WorkspaceEnvRef {
        let mut inner = self.inner.write();
        let key = WorkspaceEnvRegistryKey {
            name,
            platform,
            spec,
        };
        if let Some(existing) = inner.refs_by_key.get(&key) {
            return existing.clone();
        }

        let id = WorkspaceEnvId(
            u32::try_from(inner.entries.len()).expect("too many workspace envs allocated"),
        );
        inner.entries.push(Arc::new(key.spec.clone()));
        let env_ref = WorkspaceEnvRef::new(id, key.name.clone(), key.platform);
        inner.refs_by_key.insert(key, env_ref.clone());
        env_ref
    }

    /// Look up the spec for an id. Panics if the id was never allocated
    /// by this registry. Only [`allocate`](Self::allocate) can mint a
    /// [`WorkspaceEnvRef`], so a valid ref always has a corresponding
    /// entry.
    pub fn get(&self, id: WorkspaceEnvId) -> Arc<EnvironmentSpec> {
        self.inner
            .read()
            .entries
            .get(id.as_index())
            .expect("allocated WorkspaceEnvId must have a registry entry")
            .clone()
    }
}

impl Default for WorkspaceEnvRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Ergonomic access to the registry from compute bodies:
/// `ctx.global_data().workspace_env_registry()`.
pub trait HasWorkspaceEnvRegistry {
    fn workspace_env_registry(&self) -> &Arc<WorkspaceEnvRegistry>;
}

impl HasWorkspaceEnvRegistry for DataStore {
    fn workspace_env_registry(&self) -> &Arc<WorkspaceEnvRegistry> {
        self.get::<Arc<WorkspaceEnvRegistry>>()
    }
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::Platform;
    use rattler_solve::ChannelPriority;

    use pixi_utils::variants::VariantConfig;

    use super::*;
    use crate::{BuildEnvironment, environment::EnvironmentSpec};

    fn empty_spec() -> EnvironmentSpec {
        EnvironmentSpec {
            channels: Vec::new(),
            build_environment: BuildEnvironment {
                host_platform: Platform::Linux64,
                host_virtual_packages: Vec::new(),
                build_platform: Platform::Linux64,
                build_virtual_packages: Vec::new(),
            },
            variants: VariantConfig::default(),
            exclude_newer: None,
            channel_priority: ChannelPriority::Strict,
        }
    }

    #[test]
    fn allocate_deduplicates_equal_requests() {
        let reg = WorkspaceEnvRegistry::new();
        let platform = Platform::Linux64;

        let a = reg.allocate("default".to_string(), platform, empty_spec());
        let b = reg.allocate("default".to_string(), platform, empty_spec());

        assert_eq!(a.id(), b.id(), "same request must reuse the existing id");
        assert_eq!(a, b, "refs with reused ids compare equal");
    }

    #[test]
    fn allocate_keeps_distinct_labels_separate() {
        let reg = WorkspaceEnvRegistry::new();
        let platform = Platform::Linux64;

        let a = reg.allocate("default".to_string(), platform, empty_spec());
        let b = reg.allocate("other".to_string(), platform, empty_spec());

        assert_ne!(
            a.id(),
            b.id(),
            "different logical labels should keep distinct ids"
        );
    }

    #[test]
    fn get_returns_spec_by_id() {
        let reg = WorkspaceEnvRegistry::new();

        let mut spec_a = empty_spec();
        spec_a.channels = vec![rattler_conda_types::ChannelUrl::from(
            url::Url::parse("https://example.com/conda-forge/").expect("valid url"),
        )];
        let ws_a = reg.allocate("default".to_string(), Platform::Linux64, spec_a.clone());
        let ws_b = reg.allocate("default".to_string(), Platform::Linux64, empty_spec());

        let got_a = reg.get(ws_a.id());
        assert_eq!(got_a.channels, spec_a.channels);

        let got_b = reg.get(ws_b.id());
        assert!(got_b.channels.is_empty());
    }
}
