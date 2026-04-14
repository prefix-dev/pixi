//! Framework-level errors returned by [`ComputeCtx::compute`](crate::ComputeCtx::compute).
//!
//! User-level errors live inside [`Key::Value`](crate::Key::Value); this
//! enum carries only the framework's own failure modes.

use std::fmt;

use crate::AnyKey;

/// An error returned by [`ComputeCtx::compute`] or [`ComputeEngine::compute`].
///
/// This enum carries only *framework*-level failure modes. User-level
/// failures (a Key's compute returning a logical error) live inside the
/// Key's [`Value`](crate::Key::Value) type, not here. A typical Key will
/// therefore define `Value = Result<T, UserError>` and fold framework
/// errors from sub-`ctx.compute` calls into its own `Value` as needed.
///
/// # Handling in a Key body
///
/// ```ignore
/// use pixi_compute_engine::{ComputeCtx, ComputeError, Key};
///
/// async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
///     match ctx.compute(&OtherKey).await {
///         Ok(value) => process(value),
///         Err(ComputeError::Cycle(stack)) => {
///             Err(format!("dependency cycle: {stack}"))
///         }
///         Err(ComputeError::Canceled) => Err("canceled".into()),
///     }
/// }
/// ```
///
/// [`ComputeCtx::compute`]: crate::ComputeCtx::compute
/// [`ComputeEngine::compute`]: crate::ComputeEngine::compute
#[derive(Debug, Clone, thiserror::Error)]
pub enum ComputeError {
    /// The requested Key participates in a dependency cycle.
    ///
    /// The attached [`CycleStack`] renders the path that closed the cycle
    /// and is useful for diagnostic messages.
    #[error("compute cycle detected: {0}")]
    Cycle(CycleStack),

    /// The underlying spawned task was aborted before it could produce a
    /// value.
    ///
    /// This happens when every subscriber to an in-flight compute drops
    /// before the compute finishes: the engine cancels the task because
    /// nobody is waiting for the value. A request that re-arrives later
    /// will spawn a fresh compute.
    #[error("compute was canceled")]
    Canceled,
}

/// A chain of Keys describing a dependency cycle.
///
/// The first element is the outermost caller currently on the compute
/// stack; subsequent elements are each requested as a dependency of the
/// previous one. The final element is the Key whose request closed the
/// cycle, and it compares equal (under [`AnyKey`] equality) to exactly one
/// of the earlier entries.
///
/// # Display format
///
/// Keys are joined with ` -> ` in the order they appear in the stack. Each
/// key is rendered through its [`AnyKey`] display, which prefixes the
/// Key's short type name.
///
/// ```
/// use pixi_compute_engine::CycleStack;
///
/// // An empty stack renders as the empty string.
/// let empty = CycleStack(Vec::new());
/// assert_eq!(format!("{empty}"), "");
/// ```
///
/// A realistic cycle for `A -> B -> A` would render as
/// `"MyKey(A) -> MyKey(B) -> MyKey(A)"`.
#[derive(Debug, Clone)]
pub struct CycleStack(pub Vec<AnyKey>);

impl fmt::Display for CycleStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for key in &self.0 {
            if !first {
                write!(f, " -> ")?;
            }
            write!(f, "{key}")?;
            first = false;
        }
        Ok(())
    }
}
