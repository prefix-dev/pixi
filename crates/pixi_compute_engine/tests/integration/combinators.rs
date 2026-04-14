//! Parallel combinator tests: `compute2`/`compute3`/`compute_many`/
//! `compute_join` and their `try_*` variants, plus `declare_*_closure`.

use std::fmt;

use futures::{FutureExt, TryFutureExt};
use pixi_compute_engine::{ComputeCtx, ComputeEngine, Key};

/// Simple numeric Key used by the parallel combinator tests.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct NumKey(u32);
impl fmt::Display for NumKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Key for NumKey {
    type Value = u32;
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        self.0 + 1
    }
}

/// Aggregator Key that exercises both `compute2` and `compute_join` by
/// summing a mix of sub-computes.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ParallelSum;
impl fmt::Display for ParallelSum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sum")
    }
}
impl Key for ParallelSum {
    type Value = u32;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let (a, b) = ctx
            .compute2(
                |ctx| ctx.compute(&NumKey(10)).boxed(),
                |ctx| ctx.compute(&NumKey(20)).boxed(),
            )
            .await;
        let rest = ctx
            .compute_join(vec![NumKey(1), NumKey(2), NumKey(3)], |ctx, k| {
                ctx.compute(&k).boxed()
            })
            .await;
        a.unwrap() + b.unwrap() + rest.into_iter().map(Result::unwrap).sum::<u32>()
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
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct Agg(bool);
    impl fmt::Display for Agg {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl Key for Agg {
        type Value = Result<u32, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let fail = self.0;
            ctx.try_compute2(
                |ctx| ctx.compute(&NumKey(1)).map_err(|_| unreachable!()).boxed(),
                move |ctx| {
                    ctx.compute(&NumKey(2))
                        .map_err(|_| unreachable!())
                        .and_then(move |v| {
                            futures::future::ready(if fail { Err("nope") } else { Ok(v) })
                        })
                        .boxed()
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
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct Triple;
    impl fmt::Display for Triple {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("triple")
        }
    }
    impl Key for Triple {
        type Value = u32;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let (a, b, c) = ctx
                .compute3(
                    |ctx| ctx.compute(&NumKey(10)).boxed(),
                    |ctx| ctx.compute(&NumKey(20)).boxed(),
                    |ctx| ctx.compute(&NumKey(30)).boxed(),
                )
                .await;
            a.unwrap() + b.unwrap() + c.unwrap()
        }
    }

    let engine = ComputeEngine::new();
    assert_eq!(engine.compute(&Triple).await.unwrap(), 11 + 21 + 31);
}

/// `try_compute3` succeeds when all three branches succeed and surfaces an
/// error from any branch otherwise.
#[tokio::test(flavor = "current_thread")]
async fn try_compute3_ok_and_err() {
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct TryTriple(u8);
    impl fmt::Display for TryTriple {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl Key for TryTriple {
        type Value = Result<u32, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let which = self.0;
            ctx.try_compute3(
                move |_ctx| async move { if which == 1 { Err("a") } else { Ok(1u32) } }.boxed(),
                move |_ctx| async move { if which == 2 { Err("b") } else { Ok(2u32) } }.boxed(),
                move |_ctx| async move { if which == 3 { Err("c") } else { Ok(3u32) } }.boxed(),
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
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct Many;
    impl fmt::Display for Many {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("many")
        }
    }
    impl Key for Many {
        type Value = u32;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let futs =
                ctx.compute_many((0..4u32).map(|i| {
                    ComputeCtx::declare_closure(move |ctx| ctx.compute(&NumKey(i)).boxed())
                }));
            futures::future::join_all(futs)
                .await
                .into_iter()
                .map(Result::unwrap)
                .sum()
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
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct Outer;
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("outer")
        }
    }
    impl Key for Outer {
        type Value = u32;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let mapper = ComputeCtx::declare_join_closure(|ctx: &mut ComputeCtx, n: u32| {
                ctx.compute(&NumKey(n)).boxed()
            });
            ctx.compute_join(vec![1u32, 2, 3], mapper)
                .await
                .into_iter()
                .map(Result::unwrap)
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
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct Joiner(bool);
    impl fmt::Display for Joiner {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl Key for Joiner {
        type Value = Result<Vec<u32>, &'static str>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let fail = self.0;
            ctx.try_compute_join(vec![1u32, 2, 3], move |ctx, n| {
                ctx.compute(&NumKey(n))
                    .map_err(|_| unreachable!())
                    .and_then(move |v| {
                        futures::future::ready(if fail && n == 2 { Err("middle") } else { Ok(v) })
                    })
                    .boxed()
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
