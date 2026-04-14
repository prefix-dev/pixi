//! The per-compute [`ComputeCtx`]. It is the channel through which a
//! running [`Key::compute`] requests dependencies.

use std::{future::Future, sync::Arc};

use futures::future::BoxFuture;

use crate::{
    AnyKey, ComputeError, CycleStack, Key,
    dedup::{Lookup, boxed_compute_future},
    engine::EngineInner,
};

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
/// That is intentional: dependency recording (added in a later phase of
/// development) requires mutation of ctx state, and `&mut self` rules out
/// the kinds of races that would make that recording non-deterministic.
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
}

impl ComputeCtx {
    pub(crate) fn new(engine: Arc<EngineInner>) -> Self {
        Self {
            engine,
            chain: Vec::new(),
        }
    }

    /// Build a handle used by the parallel combinators to produce one
    /// future per branch, each owning its own sub-ctx that inherits this
    /// ctx's cycle chain.
    fn parallel(&mut self) -> Parallel<'_> {
        Parallel {
            engine: &self.engine,
            chain: &self.chain,
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
        // Synchronous setup: cycle check + cache/spawn lookup. Doing this in
        // the calling frame (rather than inside an `async` body) lets us
        // decouple the returned future from the `&K` lifetime.
        let any_key = AnyKey::new(key.clone());
        let setup: Result<Lookup<K::Value>, ComputeError> =
            if self.chain.iter().any(|k| k == &any_key) {
                let mut stack = self.chain.clone();
                stack.push(any_key);
                Err(ComputeError::Cycle(CycleStack(stack)))
            } else {
                let child_chain = {
                    let mut c = self.chain.clone();
                    c.push(any_key);
                    c
                };
                Ok(self.engine.store.get_or_insert_with(key, || {
                    spawn_compute_future::<K>(self.engine.clone(), key.clone(), child_chain)
                }))
            };

        async move {
            match setup? {
                Lookup::Completed(value) => Ok(value),
                Lookup::InFlight(shared) => shared.await,
            }
        }
    }

    /// Run two closures concurrently, each with its own sub-ctx.
    ///
    /// Both sub-ctxes inherit the parent's cycle chain, so cycle detection
    /// keeps working across the parallel split. Each closure receives a
    /// fresh `&mut ComputeCtx` it can use for further `compute` calls.
    ///
    /// Concurrency is via [`futures::future::join`] (same-task polling);
    /// true parallelism comes from [`Self::compute`] spawning one tokio
    /// task per unique Key.
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
        let p = self.parallel();
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
        let p = self.parallel();
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
        let p = self.parallel();
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
        let p = self.parallel();
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
        let p = self.parallel();
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
        let p = self.parallel();
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
        let p = self.parallel();
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
struct Parallel<'p> {
    engine: &'p Arc<EngineInner>,
    chain: &'p Vec<AnyKey>,
}

impl Parallel<'_> {
    fn compute<F, T>(&self, func: F) -> impl Future<Output = T> + use<F, T>
    where
        F: for<'x> FnOnce(&'x mut ComputeCtx) -> BoxFuture<'x, T> + Send,
    {
        let mut ctx = ComputeCtx {
            engine: self.engine.clone(),
            chain: self.chain.clone(),
        };
        async move { func(&mut ctx).await }
    }
}

/// Build the future that will be driven by a freshly-spawned tokio task for
/// Key `K`.
///
/// Runs under the dedup store's mutex, so it must be quick. The bulk of
/// the work happens inside the spawned task, not here.
fn spawn_compute_future<K: Key>(
    engine: Arc<EngineInner>,
    key: K,
    child_chain: Vec<AnyKey>,
) -> BoxFuture<'static, Result<K::Value, ComputeError>> {
    let handle = tokio::spawn(async move {
        let mut child_ctx = ComputeCtx {
            engine: engine.clone(),
            chain: child_chain,
        };
        let value = key.compute(&mut child_ctx).await;
        // Promote to the completed cache synchronously, before returning,
        // so cancellation cannot interrupt the promotion.
        engine.store.insert_completed::<K>(&key, value.clone());
        value
    });
    boxed_compute_future(handle)
}
