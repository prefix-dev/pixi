//! Generic incremental computation engine for pixi.
//!
//! Users define [`Key`]s that describe a unit of work. The [`ComputeEngine`]
//! dedups concurrent requests for the same key, caches the resulting value,
//! detects cycles, and exposes implicit dependency tracking via
//! [`ComputeCtx::compute`].
//!
//! # Example
//!
//! A `Fib` Key that recursively computes Fibonacci numbers. The engine
//! dedups overlapping subcomputations, so each `Fib(n)` runs exactly once
//! regardless of how many parents depend on it, turning the naive
//! exponential recursion into linear work.
//!
//! ```
//! use std::fmt;
//! use futures::FutureExt;
//! use pixi_compute_engine::{ComputeCtx, ComputeEngine, Key};
//!
//! #[derive(Clone, Debug, Hash, PartialEq, Eq)]
//! struct Fib(u32);
//!
//! impl fmt::Display for Fib {
//!     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//!         write!(f, "{}", self.0)
//!     }
//! }
//!
//! impl Key for Fib {
//!     // User errors live inside `Value`: here a `Result` carrying a
//!     // static-str error. The `Result` shape also lets sub-computes be
//!     // boxed directly with `.boxed()`, with no `async move` wrapping.
//!     type Value = Result<u64, &'static str>;
//!
//!     async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
//!         let n = self.0;
//!         if n > 93 {
//!             return Err("fib overflows u64 past 93");
//!         }
//!         if n < 2 {
//!             return Ok(n as u64);
//!         }
//!         let (a, b) = ctx
//!             .compute2(
//!                 |ctx| ctx.compute(&Fib(n - 1)).boxed(),
//!                 |ctx| ctx.compute(&Fib(n - 2)).boxed(),
//!             )
//!             .await;
//!         // Outer `Result` is `ComputeError` (cycles/cancellation; impossible
//!         // here), inner is our user error.
//!         Ok(a.unwrap()? + b.unwrap()?)
//!     }
//! }
//!
//! # tokio_test::block_on(async {
//! let engine = ComputeEngine::new();
//! assert_eq!(engine.compute(&Fib(10)).await.unwrap(), Ok(55));
//!
//! // After a compute settles, the engine's dependency graph can be
//! // snapshotted via `DependencyGraph::from_engine`. The snapshot is a
//! // detached clone, so subsequent computes do not mutate it.
//! use pixi_compute_engine::DependencyGraph;
//! let graph = DependencyGraph::from_engine(&engine);
//!
//! // Render the snapshot to Graphviz `.dot` for visualization. Use
//! // `write_dot(path)` to write straight to a file, or `write_dot_to`
//! // for any `std::io::Write` (here, stderr).
//! graph.write_dot_to(&mut std::io::stderr()).unwrap();
//! # });
//! ```
//!
//! # Value clone-cheapness
//!
//! Every subscriber to a deduped compute receives its own clone of the
//! value. `Key::Value` should therefore be cheap to clone, typically an
//! `Arc<T>` or a newtype around `Arc<T>`. Returning a `Vec<u8>` or other
//! owned container as a `Value` will clone the entire payload on every
//! dedup hit.
//!
//! # Spawn-driven progress
//!
//! Each compute runs as a [`tokio::spawn`]-ed task. The cache layer uses a
//! two-tier split: in-flight tasks are tracked via weak shared-future
//! references (so they do not keep the task alive on their own), while
//! completed values are promoted into a strong-ref cache. Progress is
//! independent of subscriber polling, so callers can freely embed
//! `ctx.compute(..)` inside cancellable `tokio::select!` arms without
//! starving other subscribers.
//!
//! When the last subscriber drops, the last strong shared-future clone is
//! dropped, the underlying future drops, and an `AbortOnDrop` guard
//! cancels the spawned task. The weak entry in the in-flight map then
//! fails to upgrade on the next request, which spawns a fresh task.
//!
//! # Injected keys
//!
//! Not every value in the graph needs to be computed. An
//! [`InjectedKey`] represents external data fed into the engine via
//! [`ComputeEngine::inject`]. Computed keys can depend on injected
//! values through the normal [`ComputeCtx::compute`] call, and the
//! dependency is tracked for introspection.
//!
//! All injected values must be provided before computing any key that
//! depends on them. Requesting an injected key that has not been set
//! *panics*. Each key may be injected at most once per engine. Both
//! restrictions exist because the engine has no invalidation mechanism:
//! computed keys that already cached a result cannot be retroactively
//! updated. If you need different injected values, create a new engine.
//!
//! ```
//! use std::fmt;
//! use pixi_compute_engine::{ComputeEngine, InjectedKey};
//!
//! #[derive(Clone, Debug, Hash, PartialEq, Eq)]
//! struct DbUrl(String);
//!
//! impl fmt::Display for DbUrl {
//!     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//!         write!(f, "{}", self.0)
//!     }
//! }
//!
//! impl InjectedKey for DbUrl {
//!     type Value = String;
//! }
//!
//! let engine = ComputeEngine::new();
//! engine.inject(DbUrl("primary".into()), "postgres://localhost/mydb".into());
//!
//! // Computed keys can read this via ctx.compute(&DbUrl("primary".into()))
//! // inside their compute body.
//! ```

mod abort_on_drop;
mod any_key;
mod builder;
mod ctx;
mod engine;
mod error;
mod injected;
pub mod introspection;
mod key;
mod key_graph;
mod short_type_name;

pub use any_key::AnyKey;
pub use builder::ComputeEngineBuilder;
pub use ctx::ComputeCtx;
pub use engine::ComputeEngine;
pub use error::{ComputeError, CycleStack};
pub use injected::InjectedKey;
pub use introspection::{DependencyGraph, GraphNode, NodeState};
pub use key::{Key, StorageType};
pub use short_type_name::short_type_name;
