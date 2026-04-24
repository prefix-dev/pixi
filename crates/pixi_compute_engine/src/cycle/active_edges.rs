//! Global active-edge graph for cycle detection.
//!
//! Atomicity of [`ActiveEdges::try_add`] is load-bearing: two
//! concurrent tasks each running a separate check-then-add could each
//! observe a cycle-free graph and each add one half of a cycle,
//! silently establishing an undetected cycle. Combining the BFS and
//! the mutation under a single lock closes that window.
//!
//! # Multigraph by design
//!
//! Two sibling parallel waits in the same compute frame can both call
//! `ctx.compute(&SameDep)` with different guards in scope. They share
//! the same `(caller, target)` key pair, but each is a distinct
//! logical wait with its own notify target. Storing just one edge per
//! pair would drop one branch's notify target and, on scope exit,
//! remove the other branch's still-live wait.
//!
//! The graph therefore stores a [`Vec`] of edge records per
//! `(caller, target)` pair. Each record has a unique [`EdgeId`]
//! minted by [`ActiveEdges::try_add`] and held by the caller's
//! [`EdgeGuard`]. Removal targets exactly that record, so sibling
//! waits on the same pair are independent.
//!
//! # Edge-captured notify targets
//!
//! Each edge also carries the notify target resolved from the
//! caller's branch-local guard stack at edge-creation time. On cycle
//! detection, the BFS collects the notify targets of every edge in
//! the ring (including every parallel-wait record on a traversed
//! pair), so the detector can deliver the cycle to the exact guards
//! that were active when the cycling edges were installed. That
//! routing rule is what makes `with_cycle_guard` scoping work
//! correctly across parallel branches and across tasks: whichever
//! scope was active when a given cycling edge was installed is the
//! scope that sees the cycle.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use parking_lot::Mutex;

use crate::{AnyKey, cycle::guard::GuardHandle};

/// Opaque identity of a single logical edge in the active-edge graph.
///
/// Minted by [`ActiveEdges::try_add`] on insert and passed back to
/// [`ActiveEdges::remove`] on scope exit so that parallel waits on
/// the same `(caller, target)` pair are tracked independently.
pub(crate) type EdgeId = u64;

/// The engine-wide map of outstanding compute→compute dependency edges.
#[derive(Default)]
pub(crate) struct ActiveEdges {
    inner: Mutex<EdgeGraph>,
}

#[derive(Default)]
struct EdgeGraph {
    /// `outgoing[caller][target]` is the (non-empty) list of live
    /// edge records for the pair. A single pair may have more than
    /// one record when sibling parallel waits both depend on the
    /// same `target` from the same `caller`.
    outgoing: HashMap<AnyKey, HashMap<AnyKey, Vec<EdgeRecord>>>,
    /// Monotonically increasing counter used to mint [`EdgeId`]s.
    next_id: EdgeId,
}

struct EdgeRecord {
    id: EdgeId,
    notify: Arc<GuardHandle>,
}

/// A cycle reported by [`ActiveEdges::try_add`].
///
/// `path` lists the distinct keys on the cycle in traversal order,
/// starting with the `caller` (the key that closed the cycle) and
/// ending with the last key before the ring wraps back to `caller`:
/// `[caller, target, ...]`. Consumers reconstruct the closing edge
/// by pairing the last entry with `caller` (index 0). A self-loop
/// therefore has a single entry, `[caller]`.
///
/// `targets` is the set of notify handles to fire: the would-be
/// closing edge's target (the one passed into `try_add`) plus every
/// live edge record encountered on the reconstructed ring. Where a
/// `(caller, target)` pair in the ring has multiple records (sibling
/// parallel waits), every record's notify target is included.
#[derive(Debug)]
pub(crate) struct DetectedCycle {
    pub(crate) path: Vec<AnyKey>,
    pub(crate) targets: Vec<Arc<GuardHandle>>,
}

impl ActiveEdges {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Atomically check for a cycle and, if none is found, install
    /// a fresh edge record for `caller → target` with `notify` as
    /// its attached guard. Returns the newly-minted [`EdgeId`] so
    /// the caller can remove exactly this record on scope exit
    /// without disturbing sibling records on the same pair.
    ///
    /// On cycle, returns the detected cycle and leaves the graph
    /// unmutated. The returned `targets` include `notify` for the
    /// would-be closing edge even though that edge itself is not
    /// stored.
    pub(crate) fn try_add(
        &self,
        caller: &AnyKey,
        target: &AnyKey,
        notify: Arc<GuardHandle>,
    ) -> Result<EdgeId, DetectedCycle> {
        let mut g = self.inner.lock();

        if caller == target {
            return Err(DetectedCycle {
                path: vec![caller.clone()],
                targets: vec![notify],
            });
        }

        let mut frontier: VecDeque<AnyKey> = VecDeque::new();
        frontier.push_back(target.clone());
        // `parent_edge[node] = (prev, notifies)` records the
        // `prev → node` pair we arrived through and every live
        // record's notify target on that pair, so we can
        // reconstruct both the key path and a complete notify list
        // on close.
        let mut parent_edge: HashMap<AnyKey, (AnyKey, Vec<Arc<GuardHandle>>)> = HashMap::new();
        let mut visited: HashSet<AnyKey> = HashSet::new();
        visited.insert(target.clone());

        while let Some(node) = frontier.pop_front() {
            if let Some(edges) = g.outgoing.get(&node) {
                for (next, records) in edges {
                    if !visited.insert(next.clone()) {
                        continue;
                    }
                    let notifies: Vec<Arc<GuardHandle>> =
                        records.iter().map(|r| r.notify.clone()).collect();
                    parent_edge.insert(next.clone(), (node.clone(), notifies));
                    if next == caller {
                        return Err(reconstruct(caller, target, notify, &parent_edge));
                    }
                    frontier.push_back(next.clone());
                }
            }
        }

        let id = g.next_id;
        g.next_id += 1;
        g.outgoing
            .entry(caller.clone())
            .or_default()
            .entry(target.clone())
            .or_default()
            .push(EdgeRecord { id, notify });
        Ok(id)
    }

    /// Remove the specific edge record identified by `id` from the
    /// `caller → target` pair. Other records on the same pair are
    /// preserved. No-op if the record is not present.
    pub(crate) fn remove(&self, caller: &AnyKey, target: &AnyKey, id: EdgeId) {
        let mut g = self.inner.lock();
        let Some(edges) = g.outgoing.get_mut(caller) else {
            return;
        };
        let mut remove_target = false;
        if let Some(records) = edges.get_mut(target) {
            if let Some(pos) = records.iter().rposition(|r| r.id == id) {
                records.remove(pos);
            }
            if records.is_empty() {
                remove_target = true;
            }
        }
        if remove_target {
            edges.remove(target);
        }
        if edges.is_empty() {
            g.outgoing.remove(caller);
        }
    }
}

/// Build a [`DetectedCycle`] from BFS state when `caller` has just
/// been reached from `target`'s side.
fn reconstruct(
    caller: &AnyKey,
    target: &AnyKey,
    closing_notify: Arc<GuardHandle>,
    parent_edge: &HashMap<AnyKey, (AnyKey, Vec<Arc<GuardHandle>>)>,
) -> DetectedCycle {
    // Walk backward from `caller` through the BFS parent chain, collecting
    // both the keys and every live notify target on each traversed pair.
    // The walk ends when we reach `target`, which has no parent entry (it
    // was the BFS origin).
    let mut mid_keys: Vec<AnyKey> = Vec::new();
    let mut mid_targets: Vec<Arc<GuardHandle>> = Vec::new();
    let mut cur = caller.clone();
    while let Some((prev, edge_notifies)) = parent_edge.get(&cur) {
        mid_keys.push(cur.clone());
        for n in edge_notifies {
            mid_targets.push(n.clone());
        }
        cur = prev.clone();
    }
    mid_keys.push(target.clone());
    mid_keys.reverse();
    // mid_keys now runs [target, first_visited, ..., caller]. Drop
    // the trailing caller so the final `path` has no repeats — see
    // `DetectedCycle::path`. Readers reconstruct the closing edge by
    // pairing `path.last()` with `path.first()`.
    mid_keys.pop();
    // mid_targets ran [notifies on prev→caller pair, ..., notifies on target→first pair];
    // reversing makes it run in ring order from target toward caller.
    mid_targets.reverse();

    let mut path = Vec::with_capacity(mid_keys.len() + 1);
    path.push(caller.clone());
    path.extend(mid_keys);
    // path is now [caller, target, first_visited, ...] (no repeat)

    let mut targets = Vec::with_capacity(mid_targets.len() + 1);
    targets.push(closing_notify); // the would-be caller→target edge
    targets.extend(mid_targets);

    DetectedCycle { path, targets }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use tokio::sync::oneshot;

    use super::*;
    use crate::{ComputeCtx, Key};

    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct K(&'static str);
    impl fmt::Display for K {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }
    impl Key for K {
        type Value = ();
        async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {}
    }

    fn ak(name: &'static str) -> AnyKey {
        AnyKey::new(K(name))
    }

    fn h() -> Arc<GuardHandle> {
        let (tx, _rx) = oneshot::channel();
        Arc::new(GuardHandle::new(tx))
    }

    #[test]
    fn self_loop_detected_and_not_added() {
        let e = ActiveEdges::new();
        let notify = h();
        let err = e.try_add(&ak("a"), &ak("a"), notify.clone()).unwrap_err();
        assert_eq!(err.path, vec![ak("a")]);
        assert_eq!(err.targets.len(), 1);
        assert!(Arc::ptr_eq(&err.targets[0], &notify));
        // Guards against a regression where a failed self-loop could
        // still leave an entry behind and poison subsequent adds.
        assert!(e.try_add(&ak("a"), &ak("b"), h()).is_ok());
    }

    #[test]
    fn three_cycle_detected_in_ring_order() {
        let e = ActiveEdges::new();
        let bc = h();
        let ca = h();
        e.try_add(&ak("b"), &ak("c"), bc.clone()).unwrap();
        e.try_add(&ak("c"), &ak("a"), ca.clone()).unwrap();
        let ab = h();
        let err = e.try_add(&ak("a"), &ak("b"), ab.clone()).unwrap_err();
        assert_eq!(err.path, vec![ak("a"), ak("b"), ak("c")]);
        assert_eq!(err.targets.len(), 3);
        assert!(Arc::ptr_eq(&err.targets[0], &ab));
        assert!(Arc::ptr_eq(&err.targets[1], &bc));
        assert!(Arc::ptr_eq(&err.targets[2], &ca));
    }

    #[test]
    fn dag_has_no_cycle() {
        let e = ActiveEdges::new();
        e.try_add(&ak("a"), &ak("b"), h()).unwrap();
        e.try_add(&ak("a"), &ak("c"), h()).unwrap();
        e.try_add(&ak("b"), &ak("d"), h()).unwrap();
        e.try_add(&ak("c"), &ak("d"), h()).unwrap();
        assert!(e.try_add(&ak("d"), &ak("e"), h()).is_ok());
    }

    #[test]
    fn cycle_on_reject_does_not_add_edge() {
        // Pins the no-partial-install guarantee that callers rely
        // on: a failed `try_add` must leave the graph as it was.
        let e = ActiveEdges::new();
        let id = e.try_add(&ak("b"), &ak("a"), h()).unwrap();
        let _ = e.try_add(&ak("a"), &ak("b"), h()).unwrap_err();
        e.remove(&ak("b"), &ak("a"), id);
        assert!(e.try_add(&ak("a"), &ak("b"), h()).is_ok());
    }

    #[test]
    fn remove_clears_outgoing_set_when_empty() {
        // Empty-set cleanup matters because a leftover empty entry
        // would make `visited` see a stale source node during BFS.
        let e = ActiveEdges::new();
        let id = e.try_add(&ak("a"), &ak("b"), h()).unwrap();
        e.remove(&ak("a"), &ak("b"), id);
        assert!(e.try_add(&ak("b"), &ak("a"), h()).is_ok());
    }

    #[test]
    fn cycle_through_pair_with_multiple_records_notifies_all() {
        // Two parallel waits with different guards, both on
        // root → shared. When shared → root closes the cycle, both
        // waits' notify targets must be in the returned list.
        let e = ActiveEdges::new();
        let rec_a = h();
        let rec_b = h();
        e.try_add(&ak("root"), &ak("shared"), rec_a.clone())
            .unwrap();
        e.try_add(&ak("root"), &ak("shared"), rec_b.clone())
            .unwrap();
        let closing = h();
        let err = e
            .try_add(&ak("shared"), &ak("root"), closing.clone())
            .unwrap_err();
        assert_eq!(err.targets.len(), 3);
        assert!(err.targets.iter().any(|t| Arc::ptr_eq(t, &closing)));
        assert!(err.targets.iter().any(|t| Arc::ptr_eq(t, &rec_a)));
        assert!(err.targets.iter().any(|t| Arc::ptr_eq(t, &rec_b)));
    }

    #[test]
    fn removing_one_parallel_edge_preserves_sibling() {
        // Closes the premature-removal failure mode: removing
        // branch A's edge must leave branch B's record intact, so
        // a cycle later through the pair still notifies branch B.
        let e = ActiveEdges::new();
        let rec_a = h();
        let rec_b = h();
        let id_a = e
            .try_add(&ak("root"), &ak("shared"), rec_a.clone())
            .unwrap();
        let _id_b = e
            .try_add(&ak("root"), &ak("shared"), rec_b.clone())
            .unwrap();
        e.remove(&ak("root"), &ak("shared"), id_a);

        let closing = h();
        let err = e.try_add(&ak("shared"), &ak("root"), closing).unwrap_err();
        assert!(err.targets.iter().any(|t| Arc::ptr_eq(t, &rec_b)));
        assert!(!err.targets.iter().any(|t| Arc::ptr_eq(t, &rec_a)));
    }
}
