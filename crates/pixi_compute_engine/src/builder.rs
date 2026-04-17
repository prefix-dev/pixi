//! Builder for [`ComputeEngine`].

use std::sync::Arc;

use crate::{
    ComputeEngine, DataStore, cycle::active_edges::ActiveEdges, engine::EngineInner,
    key_graph::KeyGraph,
};

/// Builder for [`ComputeEngine`].
///
/// Obtain one via [`ComputeEngine::builder`], chain configuration
/// methods, then call [`ComputeEngineBuilder::build`] to produce the
/// engine.
///
/// # Example
///
/// ```
/// use pixi_compute_engine::ComputeEngine;
///
/// let engine = ComputeEngine::builder()
///     .sequential_branches(true)
///     .build();
/// # let _ = engine;
/// ```
#[derive(Default)]
pub struct ComputeEngineBuilder {
    sequential_branches: bool,
    global_data: DataStore,
}

impl ComputeEngineBuilder {
    /// A fresh builder with all settings at their defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// When `true`, the parallel combinators on `ComputeCtx`
    /// (`compute2`, `compute3`, `compute_join`, and friends) run their
    /// branches one at a time in mint order instead of concurrently.
    /// Defaults to `false`.
    ///
    /// This is a determinism primitive for tests: it makes sub-compute
    /// side-effect ordering reproducible regardless of tokio wake
    /// scheduling. It is not intended for production use because it
    /// serializes work that could otherwise run concurrently, reducing
    /// throughput.
    ///
    /// # Scope
    ///
    /// The setting applies **only** to sub-computes inside a single
    /// `Key::compute` frame. Multiple top-level
    /// [`ComputeEngine::compute`](crate::ComputeEngine::compute) calls
    /// joined together by the caller (e.g. with `tokio::join!`) always
    /// run concurrently regardless of this setting. If you need
    /// ordering across top-level calls, use sequential `.await`s at the
    /// call site.
    pub fn sequential_branches(mut self, value: bool) -> Self {
        self.sequential_branches = value;
        self
    }

    /// Insert a value into the engine-wide shared data store.
    ///
    /// Keys access stored values via
    /// [`ComputeCtx::global_data`](crate::ComputeCtx::global_data).
    ///
    /// # Panics
    ///
    /// Panics if a value of type `T` was already inserted.
    pub fn with_data<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.global_data.set(value);
        self
    }

    /// Build the [`ComputeEngine`].
    pub fn build(self) -> ComputeEngine {
        ComputeEngine {
            inner: Arc::new(EngineInner {
                graph: KeyGraph::default(),
                active_edges: Arc::new(ActiveEdges::new()),
                sequential_branches: self.sequential_branches,
                global_data: self.global_data,
            }),
        }
    }
}
