//! Cycle detection tests.

use std::fmt;

use futures::{FutureExt, TryFutureExt};
use pixi_compute_engine::{ComputeCtx, ComputeEngine, ComputeError, Key};

/// Which cycle shape the `CycleKey` family should form during a test.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
enum CyclePattern {
    /// `A -> A`
    SelfLoop,
    /// `A -> B -> A`
    Ping,
    /// `A -> B -> C -> B`
    Deep,
}

/// A Key that calls the next Key in `pattern` and folds any
/// `ComputeError::Cycle` into its `Value`, so tests can assert on the
/// rendered chain.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct CycleKey {
    name: char,
    pattern: CyclePattern,
}
impl fmt::Display for CycleKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}
impl Key for CycleKey {
    type Value = Result<(), String>;
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let next = match (self.pattern, self.name) {
            (CyclePattern::SelfLoop, 'A') => Some('A'),
            (CyclePattern::Ping, 'A') => Some('B'),
            (CyclePattern::Ping, 'B') => Some('A'),
            (CyclePattern::Deep, 'A') => Some('B'),
            (CyclePattern::Deep, 'B') => Some('C'),
            (CyclePattern::Deep, 'C') => Some('B'),
            _ => None,
        };
        let Some(n) = next else {
            return Ok(());
        };
        let child = CycleKey {
            name: n,
            pattern: self.pattern,
        };
        match ctx.compute(&child).await {
            Ok(inner) => inner,
            Err(ComputeError::Cycle(stack)) => Err(format!("cycle:{stack}")),
            Err(ComputeError::Canceled) => Err("canceled".into()),
        }
    }
}

/// Render the error produced when a `CycleKey` chain closes a cycle.
/// Shared helper for the cycle snapshot tests.
async fn render_cycle(pattern: CyclePattern) -> String {
    ComputeEngine::new()
        .compute(&CycleKey { name: 'A', pattern })
        .await
        .unwrap()
        .unwrap_err()
}

/// `A -> A` is detected; the snapshot pins the self-loop render.
#[tokio::test(flavor = "current_thread")]
async fn direct_cycle() {
    insta::assert_snapshot!(
        render_cycle(CyclePattern::SelfLoop).await,
        @"cycle:CycleKey(A) -> CycleKey(A)"
    );
}

/// `A -> B -> A` is detected; the snapshot also serves as the
/// `CycleStack` `Display` format contract (entries joined by ` -> `).
#[tokio::test(flavor = "current_thread")]
async fn indirect_cycle() {
    insta::assert_snapshot!(
        render_cycle(CyclePattern::Ping).await,
        @"cycle:CycleKey(A) -> CycleKey(B) -> CycleKey(A)"
    );
}

/// `A -> B -> C -> B` closes on `B`; snapshot shows `B` appearing twice
/// (the second occurrence is the edge that closed the cycle).
#[tokio::test(flavor = "current_thread")]
async fn deep_cycle() {
    insta::assert_snapshot!(
        render_cycle(CyclePattern::Deep).await,
        @"cycle:CycleKey(A) -> CycleKey(B) -> CycleKey(C) -> CycleKey(B)"
    );
}

/// Cycle detection inherits the parent's chain when splitting across
/// parallel sub-ctxes, so a cycle reachable only through a `compute2` arm is
/// still caught.
#[tokio::test(flavor = "current_thread")]
async fn cycle_detection_across_sub_ctx() {
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct Outer;
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("outer")
        }
    }
    impl Key for Outer {
        type Value = Result<(), String>;
        async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
            let (_l, r) = ctx
                .compute2(
                    |_ctx| futures::future::ready(Ok::<(), String>(())).boxed(),
                    |ctx| {
                        ctx.compute(&Outer)
                            .unwrap_or_else(|e| match e {
                                ComputeError::Cycle(s) => Err(format!("cycle:{s}")),
                                ComputeError::Canceled => Err("canceled".into()),
                            })
                            .boxed()
                    },
                )
                .await;
            r
        }
    }

    let engine = ComputeEngine::new();
    let msg = engine.compute(&Outer).await.unwrap().unwrap_err();
    assert!(msg.contains("Outer(outer)"), "got: {msg}");
}
