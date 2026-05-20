use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use derive_more::Display;
use rattler_conda_types::Platform;

/// Dense `u32` id allocated by
/// [`WorkspaceEnvRegistry`](super::WorkspaceEnvRegistry). The id directly
/// indexes the registry's vec of specs.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd, Display)]
#[display("{_0}")]
pub struct WorkspaceEnvId(pub(super) u32);

impl WorkspaceEnvId {
    #[inline]
    pub(super) fn as_index(self) -> usize {
        self.0 as usize
    }
}

/// Stable handle to an environment within a workspace. Arc-wrapped so
/// clones are cheap.
///
/// `Hash` / `PartialEq` / `Eq` delegate to the `id` only: name and
/// platform are cosmetic labels that travel with the ref for display
/// but are not part of identity. Two refs with the same id compare
/// equal regardless of labels; two refs with different ids never
/// compare equal even if they share labels.
///
/// Only [`WorkspaceEnvRegistry::allocate`](super::WorkspaceEnvRegistry::allocate)
/// can mint a `WorkspaceEnvRef`; the constructor is crate-private.
#[derive(Clone, Debug, Display)]
#[display("{}@{}", _0.name, _0.platform)]
pub struct WorkspaceEnvRef(Arc<WorkspaceEnvInner>);

#[derive(Debug)]
pub(super) struct WorkspaceEnvInner {
    pub(super) id: WorkspaceEnvId,
    pub(super) name: String,
    pub(super) platform: Platform,
}

impl WorkspaceEnvRef {
    pub(super) fn new(id: WorkspaceEnvId, name: String, platform: Platform) -> Self {
        Self(Arc::new(WorkspaceEnvInner { id, name, platform }))
    }

    #[inline]
    pub fn id(&self) -> WorkspaceEnvId {
        self.0.id
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.0.name
    }

    #[inline]
    pub fn platform(&self) -> Platform {
        self.0.platform
    }
}

impl Hash for WorkspaceEnvRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}

impl PartialEq for WorkspaceEnvRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for WorkspaceEnvRef {}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;

    use rattler_conda_types::Platform;

    use super::*;

    fn mk(id: u32, name: &str, platform: Platform) -> WorkspaceEnvRef {
        WorkspaceEnvRef::new(WorkspaceEnvId(id), name.to_string(), platform)
    }

    fn hash_of(ws: &WorkspaceEnvRef) -> u64 {
        let mut h = DefaultHasher::new();
        ws.hash(&mut h);
        h.finish()
    }

    #[test]
    fn different_ids_same_labels_are_unequal() {
        let a = mk(0, "default", Platform::Linux64);
        let b = mk(1, "default", Platform::Linux64);
        assert_ne!(a, b);
        assert_ne!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn same_id_different_labels_are_equal() {
        let a = mk(7, "default", Platform::Linux64);
        let b = mk(7, "other", Platform::OsxArm64);
        assert_eq!(a, b);
        assert_eq!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn display_formats_name_at_platform() {
        let ws = mk(0, "default", Platform::Linux64);
        assert_eq!(ws.to_string(), "default@linux-64");
    }
}
