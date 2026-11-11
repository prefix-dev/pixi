//! Parallel combinator tests: `compute2`/`compute3`/`compute_many`/
//! `compute_join` and their `try_*` variants, plus `declare_*_closure`.

use derive_more::Display;
use pixi_compute_engine::{ComputeCtx, ComputeEngine, Key};

/// Simple numeric Key used by the parallel combinator tests.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
struct NumKey(u32);
impl Key for NumKey {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.0 + 1
    }
}

/// Aggregator Key that exercises both `compute2` and `compute_join` by
/// summing a mix of sub-computes.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("sum")]
struct ParallelSum;
impl Key for ParallelSum {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let (a, b) = ctx
            .compute2(
                async |ctx| ctx.compute(&NumKey(10)).await,
                async |ctx| ctx.compute(&NumKey(20)).await,
            )
            .await;
        let rest = ctx
            .compute_join(vec![NumKey(1), NumKey(2), NumKey(3)], async |ctx, k| {
                ctx.compute(&k).await
            })
            .await;
        a + b + rest.into_iter().sum::<u32>()
    }
}

/// `compute2` + `compute_join` both deliver the expected sub-values.
#[tokio::test(flavor = "current_thread")]
async fn parallel_combinators() {
    let engine = ComputeEngine::new();
    // (10+1) + (20+1) + (1+1) + (2+1) + (3+1) = 11 + 21 + 2 + 3 + 4 = 41
    assert_eq!(engine.compute(&ParallelSum).await.unwrap(), 41);
}

/// `try_compute2` succeeds when both branches succeed and surfaces the first
/// error otherwise.
#[tokio::test(flavor = "current_thread")]
async fn try_compute2_ok_and_err() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("{_0}")]
    struct Agg(bool);
    impl Key for Agg {
        type Value = Result<u32, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let fail = self.0;
            ctx.try_compute2(
                async |ctx| Ok::<u32, &'static str>(ctx.compute(&NumKey(1)).await),
                async move |ctx| {
                    let v = ctx.compute(&NumKey(2)).await;
                    if fail { Err("nope") } else { Ok(v) }
                },
            )
            .await
            .map(|(a, b)| a + b)
        }
    }

    let engine = ComputeEngine::new();
    assert_eq!(engine.compute(&Agg(false)).await.unwrap(), Ok(2 + 3));
    assert_eq!(engine.compute(&Agg(true)).await.unwrap(), Err("nope"));
}

/// `compute3` resolves all three branches in parallel and returns a tuple.
#[tokio::test(flavor = "current_thread")]
async fn compute3_resolves_three() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("triple")]
    struct Triple;
    impl Key for Triple {
        type Value = u32;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let (a, b, c) = ctx
                .compute3(
                    async |ctx| ctx.compute(&NumKey(10)).await,
                    async |ctx| ctx.compute(&NumKey(20)).await,
                    async |ctx| ctx.compute(&NumKey(30)).await,
                )
                .await;
            a + b + c
        }
    }

    let engine = ComputeEngine::new();
    assert_eq!(engine.compute(&Triple).await.unwrap(), 11 + 21 + 31);
}

/// `try_compute3` succeeds when all three branches succeed and surfaces an
/// error from any branch otherwise.
#[tokio::test(flavor = "current_thread")]
async fn try_compute3_ok_and_err() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("{_0}")]
    struct TryTriple(u8);
    impl Key for TryTriple {
        type Value = Result<u32, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let which = self.0;
            ctx.try_compute3(
                async move |_ctx| if which == 1 { Err("a") } else { Ok(1u32) },
                async move |_ctx| if which == 2 { Err("b") } else { Ok(2u32) },
                async move |_ctx| if which == 3 { Err("c") } else { Ok(3u32) },
            )
            .await
            .map(|(a, b, c)| a + b + c)
        }
    }

    let engine = ComputeEngine::new();
    assert_eq!(engine.compute(&TryTriple(0)).await.unwrap(), Ok(6));
    assert_eq!(engine.compute(&TryTriple(2)).await.unwrap(), Err("b"));
}

/// `compute_many` builds an independent future per input closure; callers
/// can drive them however they like.
#[tokio::test(flavor = "current_thread")]
async fn compute_many_builds_independent_futures() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("many")]
    struct Many;
    impl Key for Many {
        type Value = u32;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let futs = ctx.compute_many((0..4u32).map(|i| {
                ComputeCtx::declare_closure(async move |ctx: &mut ComputeCtx| {
                    ctx.compute(&NumKey(i)).await
                })
            }));
            futures::future::join_all(futs).await.into_iter().sum()
        }
    }

    let engine = ComputeEngine::new();
    // (0+1) + (1+1) + (2+1) + (3+1) = 10
    assert_eq!(engine.compute(&Many).await.unwrap(), 10);
}

/// `declare_join_closure` pins the HRTB on a mapper declared outside the
/// `compute_join` call site.
#[tokio::test(flavor = "current_thread")]
async fn declare_join_closure_pins_hrtb() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("outer")]
    struct Outer;
    impl Key for Outer {
        type Value = u32;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let mapper = ComputeCtx::declare_join_closure(async |ctx: &mut ComputeCtx, n: u32| {
                ctx.compute(&NumKey(n)).await
            });
            ctx.compute_join(vec![1u32, 2, 3], mapper)
                .await
                .into_iter()
                .sum()
        }
    }

    let engine = ComputeEngine::new();
    assert_eq!(engine.compute(&Outer).await.unwrap(), 2 + 3 + 4);
}

/// `try_compute_join` returns all values on success and short-circuits on
/// the first error.
#[tokio::test(flavor = "current_thread")]
async fn try_compute_join_ok_and_err() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("{_0}")]
    struct Joiner(bool);
    impl Key for Joiner {
        type Value = Result<Vec<u32>, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let fail = self.0;
            ctx.try_compute_join(vec![1u32, 2, 3], async move |ctx, n| {
                let v = ctx.compute(&NumKey(n)).await;
                if fail && n == 2 { Err("middle") } else { Ok(v) }
            })
            .await
        }
    }

    let engine = ComputeEngine::new();
    assert_eq!(
        engine.compute(&Joiner(false)).await.unwrap(),
        Ok(vec![2, 3, 4])
    );
    assert_eq!(engine.compute(&Joiner(true)).await.unwrap(), Err("middle"));
}
