//! Tests for [`InjectedKey`]: inject/read round-trip, `ctx.compute`
//! integration, dep recording, re-inject panic, and introspection state.

use derive_more::Display;
use futures::FutureExt;
use pixi_compute_engine::{
    ComputeCtx, ComputeEngine, DependencyGraph, InjectedKey, Key, NodeState,
};

/// A minimal injected key for testing.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct Param(u32);

impl InjectedKey for Param {
    type Value = u64;
}

// -- inject / read round-trip ------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn inject_then_read() {
    let engine = ComputeEngine::new();
    engine.inject(Param(1), 42);
    assert_eq!(engine.read(&Param(1)).unwrap(), 42);
}

#[tokio::test(flavor = "current_thread")]
async fn inject_then_compute() {
    let engine = ComputeEngine::new();
    engine.inject(Param(1), 42);
    assert_eq!(engine.compute(&Param(1)).await.unwrap(), 42);
}

// -- read-before-inject ------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn read_before_inject_returns_none() {
    let engine = ComputeEngine::new();
    assert!(engine.read(&Param(1)).is_none());
}

#[tokio::test(flavor = "current_thread")]
#[should_panic(expected = "injected key not set")]
async fn compute_before_inject_panics() {
    let engine = ComputeEngine::new();
    let _ = engine.compute(&Param(1)).await;
}

// -- re-inject panics --------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[should_panic(expected = "injected key already set")]
async fn re_inject_panics() {
    let engine = ComputeEngine::new();
    engine.inject(Param(1), 1);
    engine.inject(Param(1), 2);
}

// -- dep recording -----------------------------------------------------------

/// A computed Key that reads an injected Param.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct Reader(u32);

impl Key for Reader {
    type Value = u64;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let val = ctx.compute(&Param(self.0)).await.unwrap();
        val + 1
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dep_from_compute_to_injected_is_recorded() {
    let engine = ComputeEngine::new();
    engine.inject(Param(3), 10);
    assert_eq!(engine.compute(&Reader(3)).await.unwrap(), 11);

    let graph = DependencyGraph::from_engine(&engine);

    // Reader(3) should have Param(3) as a dep.
    let edge_for_reader: Vec<String> = graph
        .edges()
        .find(|(parent, _)| parent.to_string() == "Reader(3)")
        .expect("Reader(3) should have edges")
        .1
        .iter()
        .map(|k| k.to_string())
        .collect();
    assert_eq!(edge_for_reader, vec!["Param(3)"]);
}

// -- introspection -----------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn introspection_shows_injected_state() {
    let engine = ComputeEngine::new();
    engine.inject(Param(5), 100);
    // Force the engine to see the node by reading it.
    let _ = engine.compute(&Param(5)).await.unwrap();

    let graph = DependencyGraph::from_engine(&engine);
    let injected_nodes: Vec<(&str, NodeState)> = graph
        .nodes()
        .filter(|n| n.state == NodeState::Injected)
        .map(|n| (n.type_name, n.state))
        .collect();
    assert_eq!(injected_nodes, vec![("Param", NodeState::Injected)]);
}

#[tokio::test(flavor = "current_thread")]
async fn injected_node_has_empty_deps() {
    let engine = ComputeEngine::new();
    engine.inject(Param(2), 50);
    let _ = engine.compute(&Param(2)).await.unwrap();

    let graph = DependencyGraph::from_engine(&engine);
    // The injected node itself should have no deps.
    let param_edges = graph
        .edges()
        .find(|(parent, _)| parent.to_string() == "Param(2)");
    assert!(
        param_edges.is_none(),
        "injected node should have no edges, got: {param_edges:?}",
    );
}

#[cfg(feature = "serde")]
#[tokio::test(flavor = "current_thread")]
async fn introspection_json_includes_injected_state() {
    let engine = ComputeEngine::new();
    engine.inject(Param(1), 42);
    engine.inject(Param(2), 99);
    // Trigger graph population.
    let _ = engine.compute(&Param(1)).await.unwrap();
    let _ = engine.compute(&Param(2)).await.unwrap();

    let graph = DependencyGraph::from_engine(&engine);
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();

    let nodes = json["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    for node in nodes {
        assert_eq!(node["state"], "Injected");
    }
}

// -- mixed graph: computed + injected ----------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn mixed_graph_computed_and_injected() {
    let engine = ComputeEngine::new();
    engine.inject(Param(1), 10);
    assert_eq!(engine.compute(&Reader(1)).await.unwrap(), 11);

    let graph = DependencyGraph::from_engine(&engine);
    let states: Vec<(String, NodeState)> = graph
        .nodes()
        .map(|n| (n.key.to_string(), n.state))
        .collect();
    assert_eq!(
        states,
        vec![
            ("Param(1)".to_string(), NodeState::Injected),
            ("Reader(1)".to_string(), NodeState::Completed),
        ]
    );
}

/// Dot output for a mixed computed + injected graph. Locks the
/// rendering so future changes to node ordering or edge format are
/// caught.
#[tokio::test(flavor = "current_thread")]
async fn mixed_graph_dot_snapshot() {
    let engine = ComputeEngine::new();
    engine.inject(Param(1), 10);
    engine.inject(Param(2), 20);
    // Reader depends on a single Param; compute both readers.
    let _ = engine.compute(&Reader(1)).await.unwrap();
    let _ = engine.compute(&Reader(2)).await.unwrap();

    let graph = DependencyGraph::from_engine(&engine);
    let mut buf = Vec::new();
    graph.write_dot_to(&mut buf).unwrap();
    let dot = String::from_utf8(buf).unwrap();

    insta::assert_snapshot!(dot, @r#"
    digraph deps {
        "Param(1)" [label="Param(1)"];
        "Param(2)" [label="Param(2)"];
        "Reader(1)" [label="Reader(1)"];
        "Reader(2)" [label="Reader(2)"];
        "Reader(1)" -> "Param(1)";
        "Reader(2)" -> "Param(2)";
    }
    "#);
}

// -- parallel reads of injected key ------------------------------------------

/// A computed Key that reads two injected Params in parallel.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct ParallelInjectedReader(u32);

impl Key for ParallelInjectedReader {
    type Value = u64;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let n = self.0;
        let (a, b) = ctx
            .compute2(
                |ctx| ctx.compute(&Param(n)).boxed(),
                |ctx| ctx.compute(&Param(n + 1)).boxed(),
            )
            .await;
        a.unwrap() + b.unwrap()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn parallel_reads_of_injected_keys() {
    let engine = ComputeEngine::new();
    engine.inject(Param(10), 100);
    engine.inject(Param(11), 200);
    assert_eq!(
        engine.compute(&ParallelInjectedReader(10)).await.unwrap(),
        300
    );

    let graph = DependencyGraph::from_engine(&engine);
    let mut deps: Vec<String> = graph
        .edges()
        .find(|(p, _)| p.to_string() == "ParallelInjectedReader(10)")
        .expect("should have edges")
        .1
        .iter()
        .map(|k| k.to_string())
        .collect();
    deps.sort();
    assert_eq!(deps, vec!["Param(10)", "Param(11)"]);
}

// -- compile-time disjointness -----------------------------------------------

/// Verify that implementing both `Key` and `InjectedKey` on the same
/// type fails to compile. The blanket `impl<K: InjectedKey> Key for K`
/// conflicts with a manual `Key` impl.
///
/// ```compile_fail
/// use std::fmt;
/// use pixi_compute_engine::{ComputeCtx, InjectedKey, Key};
///
/// #[derive(Clone, Debug, Hash, PartialEq, Eq)]
/// struct Dual(u32);
///
/// impl fmt::Display for Dual {
///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///         write!(f, "{}", self.0)
///     }
/// }
///
/// impl InjectedKey for Dual {
///     type Value = u32;
/// }
///
/// // This should fail: conflicting implementations of `Key`.
/// impl Key for Dual {
///     type Value = u32;
///     async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
///         self.0
///     }
/// }
/// ```
const _: () = ();

// -- concurrent injection ----------------------------------------------------

/// Inject multiple keys concurrently from separate tasks. The per-type
/// locking must serialize access without deadlock or data corruption.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_injection_of_distinct_keys() {
    let engine = ComputeEngine::new();
    let mut handles = Vec::new();
    for i in 0..64u32 {
        let e = engine.clone();
        handles.push(tokio::spawn(async move {
            e.inject(Param(i), i as u64 * 10);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    for i in 0..64u32 {
        assert_eq!(engine.compute(&Param(i)).await.unwrap(), i as u64 * 10);
    }
}

// -- sequential_branches + injected keys -------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn sequential_branches_with_injected_keys() {
    let engine = ComputeEngine::builder().sequential_branches(true).build();
    engine.inject(Param(10), 100);
    engine.inject(Param(11), 200);
    assert_eq!(
        engine.compute(&ParallelInjectedReader(10)).await.unwrap(),
        300,
    );
}

// -- multiple injected key types ---------------------------------------------

/// A second injected key type to confirm per-type sharding works.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct Setting(String);

impl InjectedKey for Setting {
    type Value = String;
}

#[tokio::test(flavor = "current_thread")]
async fn multiple_injected_key_types() {
    let engine = ComputeEngine::new();
    engine.inject(Param(1), 42);
    engine.inject(Setting("color".into()), "blue".into());

    assert_eq!(engine.compute(&Param(1)).await.unwrap(), 42);
    assert_eq!(
        engine.compute(&Setting("color".into())).await.unwrap(),
        "blue",
    );

    let graph = DependencyGraph::from_engine(&engine);
    assert_eq!(graph.len(), 2);
    let type_names: Vec<&str> = graph.nodes().map(|n| n.type_name).collect();
    assert!(type_names.contains(&"Param"));
    assert!(type_names.contains(&"Setting"));
}

// -- engine.read + engine.compute consistency --------------------------------

#[tokio::test(flavor = "current_thread")]
async fn read_and_compute_return_same_value() {
    let engine = ComputeEngine::new();
    engine.inject(Param(99), 777);

    let via_read = engine.read(&Param(99)).unwrap();
    let via_compute = engine.compute(&Param(99)).await.unwrap();
    assert_eq!(via_read, via_compute);
}
