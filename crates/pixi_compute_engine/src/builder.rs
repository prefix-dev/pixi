//! Builder for [`ComputeEngine`].

use std::sync::Arc;

use crate::{ComputeEngine, engine::EngineInner, key_graph::KeyGraph};

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
#[derive(Clone, Debug, Default)]
pub struct ComputeEngineBuilder {
    sequential_branches: bool,
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
    /// scheduling. It is not intended for production use.
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

    /// Build the [`ComputeEngine`].
    pub fn build(self) -> ComputeEngine {
        ComputeEngine {
            inner: Arc::new(EngineInner {
                graph: KeyGraph::default(),
                sequential_branches: self.sequential_branches,
            }),
        }
    }
}
