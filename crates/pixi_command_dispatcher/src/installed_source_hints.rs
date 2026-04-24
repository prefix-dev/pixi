//! Deduplicated, depth-unified view of `installed` source-record hints
//! passed to a `SolvePixiEnvironmentKey`.
//!
//! A pixi solve receives an `installed: Vec<UnresolvedPixiRecord>`
//! stability hint from its caller. When that list carries multiple
//! entries for the same source package, or when a nested source
//! record inside a top-level record's `build_packages` /
//! `host_packages` carries a hint that disagrees with the top-level
//! view, the recursive walk (top-level SPEK -> `ResolveSourcePackageKey`
//! -> nested SPEK) ends up calling the same `ResolveSourcePackageKey`
//! with different `installed_build_packages` / `installed_host_packages`
//! inputs, producing divergent cache entries for what is logically
//! the same package.
//!
//! [`InstalledSourceHints`] flattens the tree once at the top and keys
//! hints on `(PackageName, SourceLocationSpec)`. The same `Arc` of this
//! type flows unchanged through every nested SPEK, so every layer of the
//! walk looks up the same canonical hint for a given package.
//!
//! Same `(name, source_location)` duplicates collapse to one canonical
//! representative via a deterministic sort; different source identities
//! for the same name (`foo` from path and `foo` from git) coexist as
//! distinct entries.

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::{Arc, OnceLock},
};

use pixi_record::{PinnedBuildSourceSpec, PinnedSourceSpec, UnresolvedPixiRecord};
use pixi_spec::SourceLocationSpec;
use rattler_conda_types::PackageName;

use crate::PtrArc;

/// Flattened + canonicalized source-record hints for a pixi solve.
#[derive(Debug, Default, Clone)]
pub struct InstalledSourceHints {
    by_key: HashMap<(PackageName, SourceLocationSpec), InstalledSourceHint>,
}

/// The canonical install hint for one `(PackageName, SourceLocationSpec)`
/// pair: which pin represents it in this environment, and which build /
/// host package sets to seed the nested solves with.
///
/// Package slices are stored as `Arc<[_]>` so callers can clone the
/// handle cheaply without re-allocating the backing buffer.
#[derive(Debug, Clone)]
pub struct InstalledSourceHint {
    pub manifest_source: PinnedSourceSpec,
    pub build_source: Option<PinnedBuildSourceSpec>,
    pub build_packages: Arc<[UnresolvedPixiRecord]>,
    pub host_packages: Arc<[UnresolvedPixiRecord]>,
}

impl InstalledSourceHints {
    /// A shared `Arc` around an empty `InstalledSourceHints`. Callers
    /// that want "no hints" (e.g. a fresh solve, or a test with no
    /// prior resolution) should use this so their pointer compares
    /// equal to every other such caller. `PtrArc::default` is wired
    /// to it via [`Default`] below.
    pub fn empty_arc() -> Arc<Self> {
        static EMPTY: OnceLock<Arc<InstalledSourceHints>> = OnceLock::new();
        Arc::clone(EMPTY.get_or_init(|| Arc::new(InstalledSourceHints::default())))
    }

    /// Walk `installed` recursively, collapsing duplicates by
    /// `(name, source_location)` and picking a canonical representative
    /// per group.
    pub fn from_records(installed: &[UnresolvedPixiRecord]) -> Self {
        let mut candidates: HashMap<(PackageName, SourceLocationSpec), Vec<InstalledSourceHint>> =
            HashMap::new();
        collect(installed, &mut candidates);

        let by_key = candidates
            .into_iter()
            .map(|(key, mut hints)| {
                hints.sort_by(canonical_order);
                let canonical = hints.into_iter().next().expect("non-empty by construction");
                (key, canonical)
            })
            .collect();

        Self { by_key }
    }

    pub fn get(
        &self,
        name: &PackageName,
        location: &SourceLocationSpec,
    ) -> Option<&InstalledSourceHint> {
        self.by_key.get(&(name.clone(), location.clone()))
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

impl Default for PtrArc<InstalledSourceHints> {
    /// Shares a singleton empty `InstalledSourceHints` so that two
    /// independently-constructed "no hints" specs compare equal under
    /// `PtrArc`'s pointer-identity `Eq`. Without this, every
    /// `PtrArc::new(Arc::new(InstalledSourceHints::default()))` would
    /// allocate a fresh `Arc` and defeat dedup at the Key boundary.
    fn default() -> Self {
        PtrArc::new(InstalledSourceHints::empty_arc())
    }
}

fn collect(
    records: &[UnresolvedPixiRecord],
    out: &mut HashMap<(PackageName, SourceLocationSpec), Vec<InstalledSourceHint>>,
) {
    for record in records {
        let Some(source) = record.as_source() else {
            continue;
        };
        let key = (
            source.name().clone(),
            SourceLocationSpec::from(source.manifest_source.clone()),
        );
        out.entry(key).or_default().push(InstalledSourceHint {
            manifest_source: source.manifest_source.clone(),
            build_source: source.build_source.clone(),
            build_packages: Arc::from(source.build_packages.clone()),
            host_packages: Arc::from(source.host_packages.clone()),
        });
        collect(&source.build_packages, out);
        collect(&source.host_packages, out);
    }
}

/// Deterministic ordering used to pick a canonical `InstalledSourceHint`
/// among duplicates. The choice is arbitrary but must be stable so that
/// input order does not change the solve result.
fn canonical_order(a: &InstalledSourceHint, b: &InstalledSourceHint) -> std::cmp::Ordering {
    let manifest = a
        .manifest_source
        .to_string()
        .cmp(&b.manifest_source.to_string());
    if manifest != std::cmp::Ordering::Equal {
        return manifest;
    }
    let build = a
        .build_source
        .as_ref()
        .map(ToString::to_string)
        .cmp(&b.build_source.as_ref().map(ToString::to_string));
    if build != std::cmp::Ordering::Equal {
        return build;
    }
    hash_vec(&a.build_packages)
        .cmp(&hash_vec(&b.build_packages))
        .then_with(|| hash_vec(&a.host_packages).cmp(&hash_vec(&b.host_packages)))
}

fn hash_vec(records: &Arc<[UnresolvedPixiRecord]>) -> u64 {
    let mut hasher = DefaultHasher::new();
    records.as_ref().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pixi_record::{PartialSourceRecordData, SourceRecordData, UnresolvedSourceRecord};
    use pixi_spec::{PathSourceSpec, SourceLocationSpec};
    use rattler_conda_types::PackageName;

    use super::*;

    fn path_location(path: &str) -> SourceLocationSpec {
        SourceLocationSpec::Path(PathSourceSpec { path: path.into() })
    }

    fn make_source(
        name: &str,
        manifest_path: &str,
        build_packages: Vec<UnresolvedPixiRecord>,
        host_packages: Vec<UnresolvedPixiRecord>,
    ) -> UnresolvedPixiRecord {
        use pixi_record::PinnedPathSpec;

        let pinned = PinnedSourceSpec::Path(PinnedPathSpec {
            path: manifest_path.into(),
        });
        let data = SourceRecordData::Partial(PartialSourceRecordData {
            name: PackageName::new_unchecked(name.to_string()),
            depends: Vec::new(),
            sources: Default::default(),
        });
        UnresolvedPixiRecord::Source(Arc::new(UnresolvedSourceRecord {
            data,
            manifest_source: pinned,
            build_source: None,
            variants: Default::default(),
            identifier_hash: None,
            build_packages,
            host_packages,
        }))
    }

    #[test]
    fn duplicate_same_key_is_deduped_deterministically() {
        let foo_a = make_source("foo", "./foo", Vec::new(), Vec::new());
        let foo_b = make_source("foo", "./foo", Vec::new(), Vec::new());

        let forward = InstalledSourceHints::from_records(&[foo_a.clone(), foo_b.clone()]);
        let reverse = InstalledSourceHints::from_records(&[foo_b, foo_a]);

        // One hint, and same hint regardless of input order.
        assert_eq!(forward.by_key.len(), 1);
        assert_eq!(reverse.by_key.len(), 1);
        let key = (
            PackageName::new_unchecked("foo".to_string()),
            path_location("./foo"),
        );
        assert_eq!(
            forward
                .get(&key.0, &key.1)
                .map(|h| h.manifest_source.to_string()),
            reverse
                .get(&key.0, &key.1)
                .map(|h| h.manifest_source.to_string()),
        );
    }

    #[test]
    fn same_name_different_sources_coexist() {
        // Same package name, different source paths -> two distinct hints.
        let from_a = make_source("foo", "./a", Vec::new(), Vec::new());
        let from_b = make_source("foo", "./b", Vec::new(), Vec::new());
        let hints = InstalledSourceHints::from_records(&[from_a, from_b]);
        assert_eq!(hints.by_key.len(), 2);
        let name = PackageName::new_unchecked("foo".to_string());
        assert!(hints.get(&name, &path_location("./a")).is_some());
        assert!(hints.get(&name, &path_location("./b")).is_some());
    }

    #[test]
    fn nested_sources_flatten_into_top_level_view() {
        // Nested source inside another source's host_packages is picked up.
        let nested = make_source("nested", "./nested", Vec::new(), Vec::new());
        let outer = make_source("outer", "./outer", Vec::new(), vec![nested]);
        let hints = InstalledSourceHints::from_records(&[outer]);
        let name = PackageName::new_unchecked("nested".to_string());
        assert!(hints.get(&name, &path_location("./nested")).is_some());
    }

    #[test]
    fn deep_nesting_is_fully_flattened() {
        // outer -> nested_1 (in host) -> nested_2 (in build). The walk
        // must descend all the way and surface every source record at
        // the top-level `by_key` map.
        let leaf = make_source("leaf", "./leaf", Vec::new(), Vec::new());
        let middle = make_source("middle", "./middle", vec![leaf], Vec::new());
        let outer = make_source("outer", "./outer", Vec::new(), vec![middle]);
        let hints = InstalledSourceHints::from_records(&[outer]);
        for name in ["outer", "middle", "leaf"] {
            assert!(
                hints
                    .get(
                        &PackageName::new_unchecked(name.to_string()),
                        &path_location(&format!("./{name}"))
                    )
                    .is_some(),
                "expected hint for {name}"
            );
        }
    }

    #[test]
    fn nested_same_name_different_location_produces_both_hints() {
        // Test 2's shape at the type level: top-level `foo` from one
        // path and a nested `foo` reached through a different path
        // land as two distinct hints (distinct `SourceLocationSpec`).
        let nested = make_source("foo", "./nested-foo", Vec::new(), Vec::new());
        let outer = make_source("bar", "./bar", Vec::new(), vec![nested]);
        let top_level_foo = make_source("foo", "./foo", Vec::new(), Vec::new());
        let hints = InstalledSourceHints::from_records(&[top_level_foo, outer]);
        let name = PackageName::new_unchecked("foo".to_string());
        assert!(hints.get(&name, &path_location("./foo")).is_some());
        assert!(hints.get(&name, &path_location("./nested-foo")).is_some());
    }

    #[test]
    fn empty_arc_is_singleton() {
        // Two independent callers that want "no hints" should end up
        // with `Arc::ptr_eq`-equal handles so their `PtrArc`-hashed
        // Keys collapse.
        let a = InstalledSourceHints::empty_arc();
        let b = InstalledSourceHints::empty_arc();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(a.is_empty());
    }

    #[test]
    fn default_ptr_arc_uses_empty_singleton() {
        use crate::PtrArc;
        let a: PtrArc<InstalledSourceHints> = Default::default();
        let b: PtrArc<InstalledSourceHints> = Default::default();
        assert_eq!(a, b, "PtrArc<InstalledSourceHints>::default() should dedup");
    }
}
