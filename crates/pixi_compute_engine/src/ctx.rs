//! The per-compute [`ComputeCtx`]. It is the channel through which a
//! running [`Key::compute`] requests dependencies.

use std::{future::Future, sync::Arc};

use futures::future::BoxFuture;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::{
    AnyKey, ComputeError, CycleStack, Key,
    engine::EngineInner,
    key_graph::{Lookup, SpawnGeneration, boxed_compute_future},
};

/// Shared dependency accumulator for a single compute frame.
///
/// All sub-ctxes minted by parallel combinators within one frame
/// share the same `DepsList` so every branch contributes to the same
/// parent's dep set. The list is moved into the `Completed` graph
/// node when the parent's compute body returns.
type DepsList = Arc<Mutex<Vec<AnyKey>>>;

/// Context passed to [`Key::compute`] so it can request dependencies.
///
/// The ctx is the only API a Key's compute body has for talking to the
/// engine. It carries:
///
/// - a handle to the engine's shared state (cache, spawn closure),
/// - the current cycle chain (the list of Keys whose computes are on the
///   stack along this branch),
/// - the machinery to spawn independently-traced sub-ctxes for parallel
///   dependency requests.
///
/// # `&mut self` on `compute`
///
/// Calling [`ComputeCtx::compute`] takes `&mut self`, which forces
/// dependency requests within a single compute frame to be serialized.
/// That is intentional: dependency recording mutates ctx state (the
/// shared dep accumulator), and `&mut self` rules out the kinds of
/// races that would make that recording non-deterministic.
/// Deterministic ordering matters because introspection and (future)
/// invalidation rely on a stable, reproducible dep list for each key.
/// For explicit parallel dependency requests, use one of the parallel
/// combinators below.
///
/// # Parallel combinators
///
/// The engine offers a fixed-arity and a variable-arity family, with
/// `Result`-aware variants for each:
///
/// | Shape                  | Plain               | Try (short-circuits)      |
/// |------------------------|---------------------|---------------------------|
/// | two branches           | [`compute2`]        | [`try_compute2`]          |
/// | three branches         | [`compute3`]        | [`try_compute3`]          |
/// | N items mapped         | [`compute_join`]    | [`try_compute_join`]      |
/// | N hand-built futures   | [`compute_many`]    | (caller joins)            |
///
/// Each closure receives its own fresh `&mut ComputeCtx` sub-ctx that
/// inherits the parent's cycle chain, so cycle detection keeps working
/// across the parallel split.
///
/// # Combinator closure shape
///
/// All parallel combinators take closures of the form
/// `for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, T> + Send`. The
/// idiomatic body is a direct `.boxed()` on a single compute call:
///
/// ```ignore
/// ctx.compute2(
///     |ctx| ctx.compute(&KeyA).boxed(),
///     |ctx| ctx.compute(&KeyB).boxed(),
/// )
/// .await
/// ```
///
/// When the closure is produced by [`Iterator::map`] or stored in a `let`
/// the `for<'x>` binder often fails to infer; use [`declare_closure`] /
/// [`declare_join_closure`] to pin it.
///
/// [`compute2`]: Self::compute2
/// [`compute3`]: Self::compute3
/// [`try_compute2`]: Self::try_compute2
/// [`try_compute3`]: Self::try_compute3
/// [`compute_many`]: Self::compute_many
/// [`compute_join`]: Self::compute_join
/// [`try_compute_join`]: Self::try_compute_join
/// [`declare_closure`]: Self::declare_closure
/// [`declare_join_closure`]: Self::declare_join_closure
pub struct ComputeCtx {
    engine: Arc<EngineInner>,
    /// Keys currently on the compute stack for this branch. Used to detect
    /// dependency cycles via a simple containment check. The last element
    /// is the Key whose `compute` is currently executing.
    chain: Vec<AnyKey>,
    /// Dependencies recorded by the currently-computing key. Shared
    /// across parallel sub-ctxes so every branch contributes to the
    /// same set; flushed into the `Completed` node when the parent's
    /// compute body returns.
    deps: DepsList,
}

impl ComputeCtx {
    pub(crate) fn new(engine: Arc<EngineInner>) -> Self {
        Self {
            engine,
            chain: Vec::new(),
            deps: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Build a handle used by the parallel combinators to produce one
    /// future per branch, each owning its own sub-ctx that inherits this
    /// ctx's cycle chain and shares this ctx's dep accumulator.
    fn parallel(&mut self) -> ParallelBuilder<'_> {
        if self.engine.sequential_branches {
            ParallelBuilder::Serial {
                engine: &self.engine,
                chain: &self.chain,
                deps: &self.deps,
                prev_done: None,
            }
        } else {
            ParallelBuilder::Concurrent {
                engine: &self.engine,
                chain: &self.chain,
                deps: &self.deps,
            }
        }
    }

    /// Request the value of another Key as a dependency of the currently
    /// running compute.
    ///
    /// Mirrors [`ComputeEngine::compute`](crate::ComputeEngine::compute)
    /// but additionally participates in cycle detection: the requested
    /// Key is checked against the current compute chain, and an
    /// [`Err(ComputeError::Cycle)`](ComputeError::Cycle) is returned if it
    /// would close one.
    ///
    /// The returned future is precisely-captured (`use<K>`): it does not
    /// borrow `key`, so callers can pass a temporary reference such as
    /// `ctx.compute(&Fib(n - 1)).boxed()` without lifetime issues.
    ///
    /// # Errors
    ///
    /// - [`ComputeError::Cycle`] if `key` is already on the current
    ///   compute chain.
    /// - [`ComputeError::Canceled`] if the underlying spawned task was
    ///   aborted before producing a value.
    ///
    /// # Example
    ///
    /// ```ignore
    /// async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
    ///     let base = ctx.compute(&BaseKey(self.0)).await.unwrap();
    ///     base * 2
    /// }
    /// ```
    pub fn compute<K: Key>(
        &mut self,
        key: &K,
    ) -> impl Future<Output = Result<K::Value, ComputeError>> + use<K> {
        // Synchronous setup: cycle check + cache/spawn lookup. Doing
        // this in the calling frame (not inside an `async` body) lets
        // us decouple the returned future from the `&K` lifetime.
        let setup = self.resolve(key);

        async move {
            match setup? {
                Lookup::Completed(value) => Ok(value),
                Lookup::InFlight(shared) => shared.await,
            }
        }
    }

    /// Synchronous core of [`compute`](Self::compute): cycle check,
    /// graph lookup/spawn, and dep recording.
    fn resolve<K: Key>(&mut self, key: &K) -> Result<Lookup<K::Value>, ComputeError> {
        let any_key = AnyKey::new(key.clone());

        // Cycle check: is this key already on the compute stack?
        if self.chain.iter().any(|k| k == &any_key) {
            let mut stack = self.chain.clone();
            stack.push(any_key);
            return Err(ComputeError::Cycle(CycleStack(stack)));
        }

        // Build the child chain for the spawned task's cycle detector.
        let mut child_chain = self.chain.clone();
        child_chain.push(any_key.clone());

        // The graph handles both computed and injected keys: on a hit
        // it returns the cached/injected value; on a miss it spawns
        // (Computed) or panics (Injected, meaning the value was never
        // provided via engine.inject()).
        let lookup = self.engine.graph.get_or_insert_with(key, |generation| {
            spawn_compute_future::<K>(self.engine.clone(), key.clone(), child_chain, generation)
        });

        self.deps.lock().push(any_key);

        Ok(lookup)
    }

    /// Run two closures concurrently, each with its own sub-ctx.
    ///
    /// Both sub-ctxes inherit the parent's cycle chain, so cycle detection
    /// keeps working across the parallel split. Each closure receives a
    /// fresh `&mut ComputeCtx` it can use for further `compute` calls.
    ///
    /// By default, concurrency is via [`futures::future::join`] (same-task
    /// polling); true parallelism comes from [`Self::compute`] spawning
    /// one tokio task per unique Key. When the owning engine is built
    /// with
    /// [`ComputeEngineBuilder::sequential_branches(true)`](crate::ComputeEngineBuilder::sequential_branches),
    /// the second closure does not start running until the first has
    /// returned.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (a, b) = ctx.compute2(
    ///     |ctx| ctx.compute(&KeyA).boxed(),
    ///     |ctx| ctx.compute(&KeyB).boxed(),
    /// ).await;
    /// // a, b: Result<Value, ComputeError>
    /// ```
    pub async fn compute2<C1, T, C2, U>(&mut self, c1: C1, c2: C2) -> (T, U)
    where
        C1: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, T> + Send,
        C2: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, U> + Send,
    {
        let mut p = self.parallel();
        futures::future::join(p.compute(c1), p.compute(c2)).await
    }

    /// Run three closures concurrently, each with its own sub-ctx.
    ///
    /// Three-arity analogue of [`compute2`](Self::compute2). For arities
    /// beyond three, use [`compute_many`](Self::compute_many) or
    /// [`compute_join`](Self::compute_join).
    pub async fn compute3<C1, T, C2, U, C3, V>(&mut self, c1: C1, c2: C2, c3: C3) -> (T, U, V)
    where
        C1: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, T> + Send,
        C2: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, U> + Send,
        C3: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, V> + Send,
    {
        let mut p = self.parallel();
        futures::future::join3(p.compute(c1), p.compute(c2), p.compute(c3)).await
    }

    /// `Result`-aware variant of [`compute2`](Self::compute2);
    /// short-circuits on the first error.
    ///
    /// As soon as either branch resolves to `Err(E)`, the other branch's
    /// future is dropped (canceling its spawned compute if nothing else
    /// is subscribed) and the error is returned. Uses
    /// [`futures::future::try_join`] internally.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (a, b) = ctx.try_compute2(
    ///     |ctx| ctx.compute(&KeyA).map_err(MyErr::from).boxed(),
    ///     |ctx| ctx.compute(&KeyB).map_err(MyErr::from).boxed(),
    /// ).await?;
    /// ```
    pub async fn try_compute2<C1, T, C2, U, E>(&mut self, c1: C1, c2: C2) -> Result<(T, U), E>
    where
        C1: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, Result<T, E>> + Send,
        C2: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, Result<U, E>> + Send,
    {
        let mut p = self.parallel();
        futures::future::try_join(p.compute(c1), p.compute(c2)).await
    }

    /// `Result`-aware variant of [`compute3`](Self::compute3);
    /// short-circuits on the first error.
    ///
    /// The first branch to resolve to `Err(E)` wins: all other in-flight
    /// branches are dropped and the error is returned. Uses
    /// [`futures::future::try_join3`] internally.
    pub async fn try_compute3<C1, T, C2, U, C3, V, E>(
        &mut self,
        c1: C1,
        c2: C2,
        c3: C3,
    ) -> Result<(T, U, V), E>
    where
        C1: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, Result<T, E>> + Send,
        C2: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, Result<U, E>> + Send,
        C3: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, Result<V, E>> + Send,
    {
        let mut p = self.parallel();
        futures::future::try_join3(p.compute(c1), p.compute(c2), p.compute(c3)).await
    }

    /// Build a vector of compute futures, one per input closure, each
    /// owning its own sub-ctx.
    ///
    /// Unlike [`compute_join`](Self::compute_join), this does *not* join
    /// the futures for you: it returns them so the caller can join with
    /// [`futures::future::join_all`],
    /// [`futures::stream::FuturesUnordered`], or any other strategy.
    ///
    /// The closures must return `BoxFuture<'x, T>` with a proper
    /// `for<'x>` HRTB; when produced by `Iterator::map`, wrap each
    /// closure in [`declare_closure`](Self::declare_closure) to pin the
    /// binder.
    ///
    /// # Driving contract under
    /// [`sequential_branches(true)`](crate::ComputeEngineBuilder::sequential_branches)
    ///
    /// Each returned future must be **either polled to completion or
    /// dropped**. Holding one alive without polling it starves its
    /// successors: the returned futures are linked so that branch N+1's
    /// closure cannot start until branch N's closure has finished, and
    /// a branch only signals completion when polled through.
    ///
    /// All standard drivers honor this ([`futures::future::join_all`],
    /// [`futures::future::try_join_all`],
    /// [`futures::stream::FuturesUnordered`], `tokio::join!`, and any
    /// unwinding path drops every branch together). The contract only
    /// matters if you split the returned `Vec` and selectively poll or
    /// retain futures.
    ///
    /// Under the concurrent default this contract is vacuous: branches
    /// are not gated on each other, so unpolled futures just never run.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let futs = ctx.compute_many(keys.into_iter().map(|k| {
    ///     ComputeCtx::declare_closure(move |ctx| ctx.compute(&k).boxed())
    /// }));
    /// let results = futures::future::join_all(futs).await;
    /// ```
    pub fn compute_many<Items, F, T>(
        &mut self,
        computes: Items,
    ) -> Vec<impl Future<Output = T> + use<Items, F, T>>
    where
        Items: IntoIterator<Item = F>,
        F: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, T> + Send,
    {
        let mut p = self.parallel();
        computes.into_iter().map(|func| p.compute(func)).collect()
    }

    /// Map each item to a compute via `mapper` and join the resulting
    /// futures concurrently.
    ///
    /// The mapper is called once per item; each invocation receives its
    /// own sub-ctx and the item (moved by value). The output order of the
    /// returned `Vec` matches the input iteration order.
    ///
    /// `Mapper: Copy` because it is called once per item. If the mapper is
    /// declared outside the call site and the closure's HRTB fails to
    /// infer, wrap it in [`declare_join_closure`](Self::declare_join_closure).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let values: Vec<Result<Val, ComputeError>> = ctx
    ///     .compute_join(keys, |ctx, k| ctx.compute(&k).boxed())
    ///     .await;
    /// ```
    pub async fn compute_join<Items, Mapper, T, R>(
        &mut self,
        items: Items,
        mapper: Mapper,
    ) -> Vec<R>
    where
        Items: IntoIterator<Item = T>,
        Mapper: for<'x> FnOnce(&'x mut ComputeCtx, T) -> BoxFuture<'x, R> + Send + Copy,
        T: Send,
    {
        let mut p = self.parallel();
        futures::future::join_all(
            items
                .into_iter()
                .map(|item| p.compute(move |ctx| mapper(ctx, item))),
        )
        .await
    }

    /// Pin the HRTB binder on a single-argument compute closure.
    ///
    /// A closure built inline inside `compute2`/`compute_many` infers its
    /// lifetime correctly, but one produced by `Iterator::map` or stored in
    /// a variable often locks to a single lifetime instead of `for<'x>`,
    /// failing to match the combinator's bound. Wrapping the closure in
    /// this identity helper forces the binder:
    ///
    /// ```ignore
    /// let futs = ctx.compute_many(keys.into_iter().map(|k| {
    ///     ComputeCtx::declare_closure(move |ctx| ctx.compute(&k).boxed())
    /// }));
    /// let values: Vec<_> = futures::future::join_all(futs).await;
    /// ```
    pub fn declare_closure<F, T>(f: F) -> F
    where
        F: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, T> + Send,
    {
        f
    }

    /// Pin the HRTB binder on a two-argument join-mapper closure, for use
    /// with [`compute_join`](Self::compute_join) /
    /// [`try_compute_join`](Self::try_compute_join) when the mapper is
    /// defined outside the call site.
    pub fn declare_join_closure<M, T, R>(m: M) -> M
    where
        M: for<'x> FnOnce(&'x mut ComputeCtx, T) -> BoxFuture<'x, R> + Send + Copy,
    {
        m
    }

    /// `Result`-aware variant of [`compute_join`](Self::compute_join);
    /// short-circuits on the first error.
    ///
    /// As soon as any mapped future resolves to `Err(E)`, the remaining
    /// in-flight branches are dropped and the error is returned. This
    /// uses [`futures::future::try_join_all`] under the hood.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let values: Vec<Val> = ctx
    ///     .try_compute_join(keys, |ctx, k| {
    ///         ctx.compute(&k).map_err(MyErr::from).boxed()
    ///     })
    ///     .await?;
    /// ```
    pub async fn try_compute_join<Items, Mapper, T, R, E>(
        &mut self,
        items: Items,
        mapper: Mapper,
    ) -> Result<Vec<R>, E>
    where
        Items: IntoIterator<Item = T>,
        Mapper: for<'x> FnOnce(&'x mut ComputeCtx, T) -> BoxFuture<'x, Result<R, E>> + Send + Copy,
        T: Send,
    {
        let mut p = self.parallel();
        futures::future::try_join_all(
            items
                .into_iter()
                .map(|item| p.compute(move |ctx| mapper(ctx, item))),
        )
        .await
    }
}

/// Handle returned by [`ComputeCtx::parallel`] for minting one future per
/// parallel branch. Each future owns its own sub-ctx that inherits the
/// parent's cycle chain, so cycle detection keeps working across the split
/// and each branch can call `ctx.compute(..)` independently.
///
/// The variant mirrors the owning engine's
/// [`sequential_branches`](crate::ComputeEngineBuilder::sequential_branches)
/// setting:
///
/// - `Concurrent` mints plain futures. Each branch runs as freely as the
///   driving combinator (`join`, `join_all`, ...) allows.
/// - `Serial` maintains a `prev_done` oneshot receiver that each
///   newly-minted branch must await *before* invoking its closure. Each
///   mint also produces a fresh sender that the branch fires after its
///   closure finishes, which unblocks the next branch. The effect is a
///   linked chain: even with `join_all` driving them concurrently, branch
///   N's closure does not start running until branch N−1's closure has
///   completed, giving deterministic FIFO sub-compute ordering.
enum ParallelBuilder<'p> {
    Concurrent {
        engine: &'p Arc<EngineInner>,
        chain: &'p Vec<AnyKey>,
        deps: &'p DepsList,
    },
    Serial {
        engine: &'p Arc<EngineInner>,
        chain: &'p Vec<AnyKey>,
        deps: &'p DepsList,
        /// The receiver the next-minted branch must await before
        /// invoking its closure. `None` initially (first branch has no
        /// predecessor).
        prev_done: Option<oneshot::Receiver<()>>,
    },
}

impl ParallelBuilder<'_> {
    fn compute<F, T>(&mut self, func: F) -> impl Future<Output = T> + use<F, T>
    where
        F: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, T> + Send,
    {
        let (engine, chain, deps) = match self {
            ParallelBuilder::Concurrent {
                engine,
                chain,
                deps,
            }
            | ParallelBuilder::Serial {
                engine,
                chain,
                deps,
                ..
            } => (*engine, *chain, *deps),
        };
        let mut ctx = ComputeCtx {
            engine: engine.clone(),
            chain: chain.clone(),
            // Sub-ctxes share the parent's dep accumulator so every
            // branch contributes to the same set.
            deps: deps.clone(),
        };
        // Serial: take the previous branch's completion receiver (this
        // branch will await it before running `func`) and install a fresh
        // receiver for the NEXT branch to await. This branch fires the
        // matching sender once its closure finishes.
        let (prev, done_tx) = match self {
            ParallelBuilder::Concurrent { .. } => (None, None),
            ParallelBuilder::Serial { prev_done, .. } => {
                let (tx, rx) = oneshot::channel();
                (prev_done.replace(rx), Some(tx))
            }
        };
        async move {
            if let Some(prev) = prev {
                // `RecvError` means the sender was dropped without firing
                // (prior branch was dropped). Proceed, because there is
                // nothing left to wait on.
                let _ = prev.await;
            }
            let value = func(&mut ctx).await;
            if let Some(done_tx) = done_tx {
                // Receiver may be gone if the next branch was dropped
                // (e.g. `try_join_all` short-circuit); send failure is not
                // an error on this path.
                let _ = done_tx.send(());
            }
            value
        }
    }
}

/// Build the future that will be driven by a freshly-spawned tokio task for
/// Key `K`.
///
/// Runs under the per-type slot's mutex, so it must be quick. The bulk
/// of the work happens inside the spawned task, not here.
///
/// `generation` is the [`SpawnGeneration`] minted by the slot for this
/// spawn. The task threads it back into [`insert_completed`], which uses
/// it to detect a stale-write race (subscribers all dropped, weak ref
/// went dangling, a fresh re-spawn replaced the slot's `InFlight` entry,
/// then this task's already-past-the-last-await tail finally runs).
fn spawn_compute_future<K: Key>(
    engine: Arc<EngineInner>,
    key: K,
    child_chain: Vec<AnyKey>,
    generation: SpawnGeneration,
) -> BoxFuture<'static, Result<K::Value, ComputeError>> {
    let handle = tokio::spawn(async move {
        let mut child_ctx = ComputeCtx {
            engine: engine.clone(),
            chain: child_chain,
            deps: Arc::new(Mutex::new(Vec::new())),
        };
        let value = key.compute(&mut child_ctx).await;
        let final_deps = std::mem::take(&mut *child_ctx.deps.lock());
        // Promote to the completed cache synchronously, before returning,
        // so cancellation cannot interrupt the promotion. The slot only
        // accepts the write if its current `InFlight` entry still has
        // our generation; otherwise we lost the race to a re-spawn and
        // the value is silently dropped.
        engine
            .graph
            .insert_completed::<K>(&key, value.clone(), final_deps, generation);
        value
    });
    boxed_compute_future(handle)
}
