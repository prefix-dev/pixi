//! Live state inspection of the compute engine's dependency graph.
//!
//! Build a snapshot with [`DependencyGraph::from_engine`]. The snapshot
//! offers iterators over nodes / edges / currently-running keys, a
//! Graphviz writer ([`DependencyGraph::write_dot`]), and a `serde`
//! `Serialize` impl for downstream tooling.
//!
//! The introspection layer uses [`AnyKey`](crate::AnyKey), so every Key
//! type is handled uniformly without per-type registration.

mod dependency_graph;
mod dot;

pub use dependency_graph::{DependencyGraph, GraphNode, NodeState};
