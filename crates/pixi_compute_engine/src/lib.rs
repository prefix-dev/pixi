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
//!     // static-str error.
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
//!                 async |ctx| ctx.compute(&Fib(n - 1)).await,
//!                 async |ctx| ctx.compute(&Fib(n - 2)).await,
//!             )
//!             .await;
//!         // `ctx.compute` returns the child's `Value` directly (no
//!         // framework-error `Result` wrapper). Our `Value` is a
//!         // `Result<u64, &str>`; propagate its error.
//!         Ok(a? + b?)
//!     }
//! }
//!
//! # tokio_test::block_on(async {
//! let engine = ComputeEngine::new();
//! assert_eq!(engine.compute(&Fib(10)).await.unwrap(), Ok(55));
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
//! # Cancellation and progress
//!
//! A compute makes progress independently of any one caller's polling.
//! Dropping a caller's future just unsubscribes that caller; if every
//! subscriber to an in-flight compute drops, the engine may cancel the
//! compute, and a later request for the same key will run it again.
//!
//! This makes `ctx.compute(..)` safe to embed inside cancellable
//! `tokio::select!` branches without starving other subscribers.
//!
//! # Cycles
//!
//! The dependency graph must be a DAG. If a Key's `compute` body requests
//! another Key whose own compute is already in-flight further up the
//! call chain, the two tasks would deadlock waiting on each other. The
//! engine detects that the moment the closing edge is requested and
//! converts the deadlock into a reportable error.
//!
//! Cycles are detected synchronously on the `ctx.compute(..)` call
//! that would close them. There is no timeout and no liveness poll:
//! if a cycle is possible, the call that closes it is the one that
//! reports it.
//!
//! ## Default: surface at the engine boundary
//!
//! If no compute body on the cycle path opts in to handling the
//! cycle, it is reported to callers of [`ComputeEngine::compute`] as
//! [`Err(ComputeError::Cycle(..))`](ComputeError::Cycle). The wrapped
//! [`CycleError`] carries the distinct keys on the cycle in order
//! as `[closing_key, next, ...]`; the closing edge is from the last
//! entry back to the first.
//!
//! Why default to a returned error rather than a panic or a `Result`
//! on every `ctx.compute`? Two reasons:
//!
//! 1. **Keep the common case ergonomic.** The vast majority of compute
//!    bodies have an acyclic dependency structure by construction.
//!    Forcing every `ctx.compute` call to match on a `Result` would
//!    clutter code with a failure mode that cannot actually occur in
//!    correctly-shaped graphs. [`ComputeCtx::compute`] therefore
//!    returns `K::Value` directly, and framework errors surface only
//!    where the engine hands a value back to the application, at
//!    [`ComputeEngine::compute`].
//! 2. **Panics are not a user-recoverable control flow.** A panic tears
//!    down the task and, depending on the runtime configuration, the
//!    whole process. Applications that want to report "this build has
//!    a cyclic dependency" to a user need a structured error they can
//!    catch, format, and continue from. Returning
//!    [`ComputeError::Cycle`] gives them that.
//!
//! ## Opting in: [`ComputeCtx::with_cycle_guard`]
//!
//! A compute body whose key may legitimately participate in a cycle
//! can wrap the suspect scope in [`ComputeCtx::with_cycle_guard`]. The
//! guard races the inner future against a cycle-notification channel.
//! When a cycle fires on this key, the scope's `select!` takes the
//! cycle arm and returns `Err(CycleError)`, which the compute body
//! can fold into its `Value`:
//!
//! ```
//! use std::fmt;
//! use pixi_compute_engine::{ComputeCtx, ComputeEngine, Key};
//!
//! #[derive(Clone, Debug, Hash, PartialEq, Eq)]
//! struct Node(u32);
//!
//! impl fmt::Display for Node {
//!     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//!         write!(f, "{}", self.0)
//!     }
//! }
//!
//! impl Key for Node {
//!     // User-visible errors live inside the Value. Here the domain
//!     // value is a `u32` and cycles are folded into a `String`.
//!     type Value = Result<u32, String>;
//!     async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
//!         // A toy graph: every `Node(n)` depends on itself. Without
//!         // a guard, `engine.compute(&Node(..))` would return
//!         // `Err(ComputeError::Cycle(..))`. With one, the compute
//!         // body sees the cycle as a normal `Err` and recovers.
//!         let me = self.0;
//!         match ctx
//!             .with_cycle_guard(async |ctx| ctx.compute(&Node(me)).await)
//!             .await
//!         {
//!             Ok(inner) => inner,
//!             Err(cycle) => Err(format!("cycle at Node({me}): {cycle}")),
//!         }
//!     }
//! }
//!
//! # tokio_test::block_on(async {
//! let engine = ComputeEngine::new();
//! let value = engine.compute(&Node(0)).await.unwrap();
//! assert!(value.unwrap_err().starts_with("cycle at Node(0)"));
//! # });
//! ```
//!
//! ## Strict scope: a guard only catches cycles its own key is on
//!
//! A [`ComputeCtx::with_cycle_guard`] scope fires only when the
//! guarding key is itself on the detected cycle path. A cycle that
//! occurs deeper in the graph, below a `ctx.compute(&X)` the caller
//! wrapped in a guard, is not rewound into the caller's guard: the
//! caller is not on the cycle, so its guard is not notified, and
//! the deep cycle still surfaces to [`ComputeEngine::compute`] as
//! [`ComputeError::Cycle`].
//!
//! A guard is a local claim that *this* key may participate in a
//! cycle and knows how to recover from one; it is not a catch-all
//! for failures anywhere below it. If you want a deep cycle caught
//! at a higher level, install a guard on the key that is actually
//! on the cycle.
//!
//! # Global data
//!
//! Keys often need access to shared resources (connection pools, caches,
//! semaphores, configuration) that are not themselves computed values.
//! The [`DataStore`] is a type-keyed container for this kind of data.
//! Values are set at engine construction time via
//! [`ComputeEngineBuilder::with_data`] and are immutable for the
//! engine's lifetime. Keys read them through
//! [`ComputeCtx::global_data`].
//!
//! The rule of thumb: if a value **affects the computed result**, it
//! belongs in the [`Key`] (so it participates in dedup and caching). If
//! it is a resource handle, observer, or optimization hint, it belongs
//! in global data.
//!
//! Downstream crates typically define extension traits on [`DataStore`]
//! for ergonomic access:
//!
//! ```ignore
//! pub trait HasGateway {
//!     fn gateway(&self) -> &Arc<Gateway>;
//! }
//!
//! impl HasGateway for DataStore {
//!     fn gateway(&self) -> &Arc<Gateway> {
//!         self.get::<Arc<Gateway>>()
//!     }
//! }
//!
//! // Inside a Key's compute body:
//! let gw = ctx.global_data().gateway();
//! ```
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
//!
//! # Introspection
//!
//! The engine's current dependency graph can be snapshotted with
//! [`DependencyGraph::from_engine`]; see the [`introspection`] module
//! for iteration, Graphviz output, and `serde::Serialize` support.

mod abort_on_drop;
mod any_key;
mod build_environment;
mod builder;
mod ctx;
mod cycle;
mod data;
mod demand;
mod engine;
mod error;
mod fingerprint;
mod injected;
mod install_pixi;
pub mod introspection;
mod key;
mod key_graph;
mod short_type_name;

pub use any_key::AnyKey;
pub use build_environment::BuildEnvironment;
pub use builder::ComputeEngineBuilder;
pub use ctx::{ComputeCtx, ParallelBuilder};
pub use cycle::CycleError;
pub use data::DataStore;
pub use demand::Demand;
pub use engine::{ComputeEngine, SpawnHook};
pub use error::ComputeError;
pub use fingerprint::EnvironmentFingerprint;
pub use injected::InjectedKey;
pub use install_pixi::{
    AllowExecuteLinkScripts, InstallPixiEnvironmentError, InstallPixiEnvironmentExt,
    InstallPixiEnvironmentResult, InstallPixiEnvironmentSpec, WrappingInstallReporter,
};
pub use introspection::{DependencyGraph, GraphNode, NodeState};
pub use key::{Key, StorageType};
pub use short_type_name::short_type_name;
