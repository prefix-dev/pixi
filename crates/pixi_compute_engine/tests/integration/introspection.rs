//! Tests for `DependencyGraph::from_engine` and the related
//! introspection surface (nodes, edges, currently-running keys, dot
//! rendering, and serde JSON output).

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use derive_more::Display;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, DataStore, DependencyGraph, Key, NodeState};
use tokio::sync::Notify;

use super::common::{BaseKey, HasTestCounter, PlusTenKey, test_counter};

/// After computing a small graph the snapshot has the expected nodes
/// and one edge from the parent to its dep.
#[tokio::test(flavor = "current_thread")]
async fn small_graph_keys_and_edges() {
    let engine = ComputeEngine::new();
    let _ = engine.compute(&PlusTenKey(5)).await.unwrap();

    let graph = DependencyGraph::from_engine(&engine);

    let key_strings: Vec<String> = graph.keys().map(|k| k.to_string()).collect();
    assert_eq!(key_strings, vec!["BaseKey(5)", "PlusTenKey(5)"]);

    let edge_strings: Vec<(String, Vec<String>)> = graph
        .edges()
        .map(|(p, c)| (p.to_string(), c.iter().map(|k| k.to_string()).collect()))
        .collect();
    assert_eq!(
        edge_strings,
        vec![("PlusTenKey(5)".to_string(), vec!["BaseKey(5)".to_string()])]
    );

    let states: Vec<(&str, NodeState)> = graph.nodes().map(|n| (n.type_name, n.state)).collect();
    assert_eq!(
        states,
        vec![
            ("BaseKey", NodeState::Completed),
            ("PlusTenKey", NodeState::Completed),
        ]
    );

    assert!(graph.keys_currently_running().next().is_none());
}

/// Test data for [`ParkedKey`].
struct ParkedKeyData {
    started: Arc<Notify>,
    release: Arc<Notify>,
    finished: Arc<AtomicBool>,
}

trait HasParkedKeyData {
    fn parked_key_data(&self) -> &ParkedKeyData;
}

impl HasParkedKeyData for DataStore {
    fn parked_key_data(&self) -> &ParkedKeyData {
        self.get::<ParkedKeyData>()
    }
}

/// While a compute is parked, the snapshot reports it as currently
/// running. After the compute completes, it reports zero in-flight.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
struct ParkedKey {
    id: u32,
}
impl Key for ParkedKey {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let data = ctx.global_data().parked_key_data();
        let started = data.started.clone();
        let release = data.release.clone();
        let finished = data.finished.clone();
        started.notify_one();
        release.notified().await;
        finished.store(true, Ordering::SeqCst);
        self.id
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keys_currently_running_reports_in_flight() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let finished = Arc::new(AtomicBool::new(false));
    let engine = ComputeEngine::builder()
        .with_data(ParkedKeyData {
            started: started.clone(),
            release: release.clone(),
            finished: finished.clone(),
        })
        .build();
    let key = ParkedKey { id: 7 };

    let e = engine.clone();
    let task = tokio::spawn(async move { e.compute(&key).await.unwrap() });

    started.notified().await;

    let snapshot = DependencyGraph::from_engine(&engine);
    let running: Vec<String> = snapshot
        .keys_currently_running()
        .map(|k| k.to_string())
        .collect();
    assert_eq!(running, vec!["ParkedKey(7)"]);
    let states: Vec<NodeState> = snapshot.nodes().map(|n| n.state).collect();
    assert_eq!(states, vec![NodeState::Computing]);

    release.notify_one();
    assert_eq!(task.await.unwrap(), 7);
    assert!(finished.load(Ordering::SeqCst));

    let snapshot = DependencyGraph::from_engine(&engine);
    assert!(snapshot.keys_currently_running().next().is_none());
    let states: Vec<NodeState> = snapshot.nodes().map(|n| n.state).collect();
    assert_eq!(states, vec![NodeState::Completed]);
}

/// Two unrelated Key types both appear in the snapshot without any
/// per-type registration in the introspection layer.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct AlphaKey(u32);
impl Key for AlphaKey {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.0
    }
}

#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct BetaKey(u32);
impl Key for BetaKey {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.0 * 10
    }
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_is_generic_over_key_types() {
    let engine = ComputeEngine::new();
    let _ = engine.compute(&AlphaKey(1)).await.unwrap();
    let _ = engine.compute(&BetaKey(2)).await.unwrap();

    let snapshot = DependencyGraph::from_engine(&engine);
    let labeled: Vec<(&str, String)> = snapshot
        .nodes()
        .map(|n| (n.type_name, n.key.to_string()))
        .collect();
    assert_eq!(
        labeled,
        vec![
            ("AlphaKey", "AlphaKey(1)".into()),
            ("BetaKey", "BetaKey(2)".into())
        ]
    );
}

/// Serializing the snapshot twice produces byte-identical output and
/// the JSON contains the expected node / edge entries.
#[cfg(feature = "serde")]
#[tokio::test(flavor = "current_thread")]
async fn serde_emits_expected_json() {
    let engine = ComputeEngine::new();
    let _ = engine.compute(&PlusTenKey(2)).await.unwrap();

    let snapshot = DependencyGraph::from_engine(&engine);
    let first = serde_json::to_string(&snapshot).unwrap();
    let second = serde_json::to_string(&snapshot).unwrap();
    assert_eq!(first, second);

    let parsed: serde_json::Value = serde_json::from_str(&first).unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["key"], "BaseKey(2)");
    assert_eq!(nodes[0]["state"], "Completed");
    assert_eq!(nodes[1]["key"], "PlusTenKey(2)");
    assert_eq!(parsed["edges"]["PlusTenKey(2)"][0], "BaseKey(2)");
    assert_eq!(parsed["running"].as_array().unwrap().len(), 0);
}

/// A non-trivial graph: Fibonacci recursion, where every `Fib(n)` is
/// computed exactly once thanks to dedup. The shape of the dot output
/// is locked in via insta so future regressions in node ordering, edge
/// ordering, or rendering are caught.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct Fib(u32);

impl Key for Fib {
    type Value = u64;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let n = self.0;
        if n < 2 {
            return n as u64;
        }
        let (a, b) = ctx
            .compute2(
                |ctx| futures::FutureExt::boxed(ctx.compute(&Fib(n - 1))),
                |ctx| futures::FutureExt::boxed(ctx.compute(&Fib(n - 2))),
            )
            .await;
        a + b
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fib_dot_snapshot() {
    let engine = ComputeEngine::new();
    assert_eq!(engine.compute(&Fib(5)).await.unwrap(), 5);

    let snapshot = DependencyGraph::from_engine(&engine);
    let mut buf = Vec::new();
    snapshot.write_dot_to(&mut buf).unwrap();
    let dot = String::from_utf8(buf).unwrap();

    insta::assert_snapshot!(dot, @r#"
    digraph deps {
        "Fib(0)" [label="Fib(0)"];
        "Fib(1)" [label="Fib(1)"];
        "Fib(2)" [label="Fib(2)"];
        "Fib(3)" [label="Fib(3)"];
        "Fib(4)" [label="Fib(4)"];
        "Fib(5)" [label="Fib(5)"];
        "Fib(2)" -> "Fib(1)";
        "Fib(2)" -> "Fib(0)";
        "Fib(3)" -> "Fib(2)";
        "Fib(3)" -> "Fib(1)";
        "Fib(4)" -> "Fib(3)";
        "Fib(4)" -> "Fib(2)";
        "Fib(5)" -> "Fib(4)";
        "Fib(5)" -> "Fib(3)";
    }
    "#);
}

/// Test data for [`ParentParkedAfterDep`].
struct ParentParkedData {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

trait HasParentParkedData {
    fn parent_parked_data(&self) -> &ParentParkedData;
}

impl HasParentParkedData for DataStore {
    fn parent_parked_data(&self) -> &ParentParkedData {
        self.get::<ParentParkedData>()
    }
}

/// A key whose compute reads a dep then parks. While the parent is
/// parked, the snapshot must not report any edge from it (deps only
/// land on the node when its compute body completes). After the parent
/// is released and finishes, the edge appears.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
struct ParentParkedAfterDep {
    id: u32,
}
impl Key for ParentParkedAfterDep {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let dep_value = ctx.compute(&BaseKey(self.id)).await;
        let data = ctx.global_data().parent_parked_data();
        data.started.notify_one();
        data.release.notified().await;
        dep_value
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deps_appear_only_after_compute_completes() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let engine = ComputeEngine::builder()
        .with_data(ParentParkedData {
            started: started.clone(),
            release: release.clone(),
        })
        .build();
    let key = ParentParkedAfterDep { id: 9 };

    let e = engine.clone();
    let task = tokio::spawn(async move { e.compute(&key).await.unwrap() });

    // Wait until the parent has read its dep and is parked at the
    // notify. Its `Completed` entry has not been written yet.
    started.notified().await;

    let snapshot = DependencyGraph::from_engine(&engine);
    assert!(
        snapshot
            .edges()
            .all(|(parent, _)| parent.to_string() != "ParentParkedAfterDep(9)"),
        "edges reported for in-flight parent: {:?}",
        snapshot
            .edges()
            .map(|(p, _)| p.to_string())
            .collect::<Vec<_>>()
    );

    release.notify_one();
    let _ = task.await.unwrap();

    let snapshot = DependencyGraph::from_engine(&engine);
    let edge_for_parent: Vec<String> = snapshot
        .edges()
        .find(|(parent, _)| parent.to_string() == "ParentParkedAfterDep(9)")
        .expect("parent should have an edge after completion")
        .1
        .iter()
        .map(|k| k.to_string())
        .collect();
    assert_eq!(edge_for_parent, vec!["BaseKey(9)"]);
}

/// A parent that reads the same dep twice. Both reads are preserved
/// in the parent's recorded dep list (we deliberately do not dedup).
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct DoubleReader(u32);
impl Key for DoubleReader {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let a = ctx.compute(&BaseKey(self.0)).await;
        let b = ctx.compute(&BaseKey(self.0)).await;
        a + b
    }
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_dep_reads_preserve_order_without_dedup() {
    let engine = ComputeEngine::new();
    let _ = engine.compute(&DoubleReader(4)).await.unwrap();

    let snapshot = DependencyGraph::from_engine(&engine);
    let edges_for_parent: Vec<String> = snapshot
        .edges()
        .find(|(parent, _)| parent.to_string() == "DoubleReader(4)")
        .expect("DoubleReader(4) should have edges")
        .1
        .iter()
        .map(|k| k.to_string())
        .collect();
    assert_eq!(edges_for_parent, vec!["BaseKey(4)", "BaseKey(4)"]);
}

/// A parent that reads two distinct deps via parallel sub-ctxes.
/// Both reads must land on the parent's dep list, proving sub-ctxes
/// share the parent's dep accumulator rather than each maintaining
/// their own (which would lose one of the deps).
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct ParallelReader(u32);
impl Key for ParallelReader {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let n = self.0;
        let (a, b) = ctx
            .compute2(
                |ctx| futures::FutureExt::boxed(ctx.compute(&BaseKey(n))),
                |ctx| futures::FutureExt::boxed(ctx.compute(&BaseKey(n + 1))),
            )
            .await;
        a + b
    }
}

#[tokio::test(flavor = "current_thread")]
async fn parallel_sub_ctxes_share_parent_dep_accumulator() {
    let engine = ComputeEngine::new();
    let _ = engine.compute(&ParallelReader(7)).await.unwrap();

    let snapshot = DependencyGraph::from_engine(&engine);
    let mut deps_for_parent: Vec<String> = snapshot
        .edges()
        .find(|(parent, _)| parent.to_string() == "ParallelReader(7)")
        .expect("ParallelReader(7) should have edges")
        .1
        .iter()
        .map(|k| k.to_string())
        .collect();
    deps_for_parent.sort();
    assert_eq!(deps_for_parent, vec!["BaseKey(7)", "BaseKey(8)"]);
}

/// A `Fib` variant that increments a counter on every entry to its
/// compute body. After computing `Fib(8)`, the counter must equal the
/// number of distinct keys (9: `Fib(0)..Fib(8)`), proving each key was
/// computed exactly once even though the recursion shape requests them
/// many times.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{n}")]
struct CountedFib {
    n: u32,
}
impl Key for CountedFib {
    type Value = u64;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        ctx.global_data()
            .test_counter()
            .fetch_add(1, Ordering::SeqCst);
        let n = self.n;
        if n < 2 {
            return n as u64;
        }
        let (a, b) = ctx
            .compute2(
                |ctx| futures::FutureExt::boxed(ctx.compute(&CountedFib { n: n - 1 })),
                |ctx| futures::FutureExt::boxed(ctx.compute(&CountedFib { n: n - 2 })),
            )
            .await;
        a + b
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fib_dedup_each_key_computed_exactly_once() {
    let counter = test_counter();
    let engine = ComputeEngine::builder().with_data(counter.clone()).build();
    assert_eq!(engine.compute(&CountedFib { n: 8 }).await.unwrap(), 21,);
    // Fib(0)..Fib(8) is 9 distinct keys, each computed exactly once.
    assert_eq!(counter.0.load(Ordering::SeqCst), 9);
}

/// Test data for [`LateWriter`].
struct LateWriterData {
    past_last_await: Arc<Notify>,
    let_finish: Arc<Notify>,
    visit_count: Arc<AtomicUsize>,
}

trait HasLateWriterData {
    fn late_writer_data(&self) -> &LateWriterData;
}

impl HasLateWriterData for DataStore {
    fn late_writer_data(&self) -> &LateWriterData {
        self.get::<LateWriterData>()
    }
}

/// A `Key` that signals when its compute body is past its last
/// `.await` and parks for a permit to run its synchronous tail. Used
/// by [`stale_late_write_does_not_clobber_respawn`] to engineer the
/// race the spawn-generation token guards against.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{id}")]
struct LateWriter {
    id: u32,
}
impl Key for LateWriter {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let data = ctx.global_data().late_writer_data();
        data.visit_count.fetch_add(1, Ordering::SeqCst);
        // Last `.await` of the compute body. Any state taken after
        // this point runs synchronously inside the spawned task.
        data.past_last_await.notify_one();
        data.let_finish.notified().await;
        self.id
    }
}

/// A regression test for the spawn-generation guard on
/// `KeyGraph::insert_completed`. Without the guard, a task whose
/// subscribers all dropped after it passed its last `.await` could
/// silently overwrite a fresh re-spawn's `InFlight` slot with its
/// stale value. With the guard, the late write is dropped and the
/// re-spawn's value is the one observed by future callers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_late_write_does_not_clobber_respawn() {
    let past_last_await = Arc::new(Notify::new());
    let let_finish = Arc::new(Notify::new());
    let visit_count = Arc::new(AtomicUsize::new(0));
    let engine = ComputeEngine::builder()
        .with_data(LateWriterData {
            past_last_await: past_last_await.clone(),
            let_finish: let_finish.clone(),
            visit_count: visit_count.clone(),
        })
        .build();
    let key = LateWriter { id: 11 };

    // T1: spawn the first compute, wait until it has reached the
    // post-`.await` synchronous tail, then drop the only subscriber.
    let e = engine.clone();
    let k = key.clone();
    let t1 = tokio::spawn(async move { e.compute(&k).await.unwrap() });
    past_last_await.notified().await;
    t1.abort();
    let _ = t1.await;

    // Release T1's parking notify. T1's tail proceeds and tries to
    // promote (with its stale generation token).
    let_finish.notify_one();
    // Yield enough times that the runtime drains T1's tail.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }

    // T2: a fresh request after the cancellation. Must spawn fresh
    // (the first visit count was T1; this one is T2) and produce a
    // value sourced from T2's own compute, not T1's stale write.
    let value = engine.compute(&key).await.unwrap();
    assert_eq!(value, 11);

    // Two visits total: T1 (cancelled before promotion) and T2 (the
    // re-spawn that actually committed).
    let visits = visit_count.load(Ordering::SeqCst);
    assert!(
        visits >= 2,
        "expected at least 2 compute invocations (T1 cancelled + T2 fresh), got {visits}"
    );
}
