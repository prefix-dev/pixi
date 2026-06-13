//! [`DependencyGraph`]: a point-in-time snapshot of the engine's graph.
//!
//! Built by walking the live [`KeyGraph`](crate::key_graph::KeyGraph) and
//! cloning out per-node state. The snapshot is the main consumer-facing
//! introspection surface; it does not retain a borrow of the engine.

use crate::{
    AnyKey, ComputeEngine,
    key_graph::{NodeRecord, RawNodeState},
};

/// The lifecycle state of a node as observed at snapshot time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(::serde::Serialize))]
pub enum NodeState {
    /// A task is currently running for this key. The value has not yet
    /// been promoted into the completed cache.
    Computing,
    /// The key's compute finished and produced a value now stored in the
    /// completed cache.
    Completed,
    /// The key's value was injected via
    /// [`ComputeEngine::inject`](crate::ComputeEngine::inject), not
    /// computed.
    Injected,
}

/// One node in a [`DependencyGraph`] snapshot.
#[derive(Clone, Debug)]
pub struct GraphNode {
    pub key: AnyKey,
    pub type_name: &'static str,
    pub state: NodeState,
}

/// A point-in-time snapshot of the compute engine's dependency graph.
///
/// Built via [`DependencyGraph::from_engine`]. The snapshot is a full
/// clone of the relevant state; subsequent computes do not mutate it.
///
/// Ordering of `nodes`, `edges`, and `running` is deterministic
/// (sorted by the [`Display`](std::fmt::Display) form of each key) so
/// tests can rely on stable snapshot output.
#[derive(Clone, Debug)]
pub struct DependencyGraph {
    nodes: Vec<GraphNode>,
    edges: Vec<(AnyKey, Vec<AnyKey>)>,
    running: Vec<AnyKey>,
}

impl DependencyGraph {
    /// Build a snapshot of the engine's current graph state.
    ///
    /// Walks the engine's key graph under per-type locks (each held
    /// only long enough to clone the per-type slot's records). The
    /// snapshot is not transactionally consistent across all node
    /// types (a concurrent compute may complete between two per-type
    /// reads), but each individual node's state is internally
    /// consistent.
    pub fn from_engine(engine: &ComputeEngine) -> Self {
        let mut records: Vec<NodeRecord> = Vec::new();
        engine
            .inner
            .graph
            .for_each_slot(|slot| slot.snapshot(&mut records));
        Self::from_records(records)
    }

    fn from_records(records: Vec<NodeRecord>) -> Self {
        let mut nodes: Vec<GraphNode> = records
            .iter()
            .map(|r| GraphNode {
                key: r.key.clone(),
                type_name: r.key.key_type_name(),
                state: match r.state {
                    RawNodeState::Computing => NodeState::Computing,
                    RawNodeState::Completed => NodeState::Completed,
                    RawNodeState::Injected => NodeState::Injected,
                },
            })
            .collect();
        nodes.sort_by_key(|n| n.key.to_string());

        let mut edges: Vec<(AnyKey, Vec<AnyKey>)> = records
            .iter()
            .filter(|r| !r.deps.is_empty())
            .map(|r| (r.key.clone(), r.deps.clone()))
            .collect();
        edges.sort_by_key(|(parent, _)| parent.to_string());

        let mut running: Vec<AnyKey> = records
            .iter()
            .filter(|r| matches!(r.state, RawNodeState::Computing))
            .map(|r| r.key.clone())
            .collect();
        running.sort_by_key(|key| key.to_string());

        Self {
            nodes,
            edges,
            running,
        }
    }

    /// All keys currently in the graph (completed + in-flight).
    pub fn keys(&self) -> impl Iterator<Item = &AnyKey> {
        self.nodes.iter().map(|n| &n.key)
    }

    /// Dependency edges, one entry per parent key.
    ///
    /// Each entry is `(parent, children)` where `children` is the
    /// ordered list of keys the parent requested via `ctx.compute(..)`,
    /// in call order. Repeated reads of the same dep are preserved as
    /// separate entries.
    pub fn edges(&self) -> impl Iterator<Item = (&AnyKey, &[AnyKey])> {
        self.edges.iter().map(|(p, c)| (p, c.as_slice()))
    }

    /// Keys whose compute task is currently running.
    pub fn keys_currently_running(&self) -> impl Iterator<Item = &AnyKey> {
        self.running.iter()
    }

    /// Typed node iterator with per-node metadata.
    pub fn nodes(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes.iter()
    }

    /// Number of nodes in the snapshot.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the snapshot contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(feature = "serde")]
mod serde {
    use super::*;
    use ::serde::{
        Serialize, Serializer,
        ser::{SerializeMap, SerializeSeq, SerializeStruct},
    };

    impl Serialize for DependencyGraph {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut s = serializer.serialize_struct("DependencyGraph", 3)?;
            s.serialize_field("nodes", &NodesView(&self.nodes))?;
            s.serialize_field("edges", &EdgesView(&self.edges))?;
            s.serialize_field("running", &KeysView(&self.running))?;
            s.end()
        }
    }

    struct KeyView<'a>(&'a AnyKey);
    impl Serialize for KeyView<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.collect_str(self.0)
        }
    }

    struct KeysView<'a>(&'a [AnyKey]);
    impl Serialize for KeysView<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut seq = s.serialize_seq(Some(self.0.len()))?;
            for k in self.0 {
                seq.serialize_element(&KeyView(k))?;
            }
            seq.end()
        }
    }

    struct NodesView<'a>(&'a [GraphNode]);
    impl Serialize for NodesView<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut seq = s.serialize_seq(Some(self.0.len()))?;
            for node in self.0 {
                seq.serialize_element(&NodeView(node))?;
            }
            seq.end()
        }
    }

    struct NodeView<'a>(&'a GraphNode);
    impl Serialize for NodeView<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut st = s.serialize_struct("GraphNode", 3)?;
            st.serialize_field("key", &KeyView(&self.0.key))?;
            st.serialize_field("type_name", self.0.type_name)?;
            st.serialize_field("state", &self.0.state)?;
            st.end()
        }
    }

    struct EdgesView<'a>(&'a [(AnyKey, Vec<AnyKey>)]);

    impl Serialize for EdgesView<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut map = s.serialize_map(Some(self.0.len()))?;
            for (parent, children) in self.0 {
                map.serialize_entry(&parent.to_string(), &KeysView(children))?;
            }
            map.end()
        }
    }
}
