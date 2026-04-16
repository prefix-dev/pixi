//! A [`tokio::task::JoinHandle`] guard that aborts the task when dropped.
//!
//! Tokio's default behavior for a dropped `JoinHandle` is to *detach*, so
//! the task keeps running orphaned. [`AbortOnDrop`] flips that: dropping
//! the guard calls [`JoinHandle::abort`], tying the spawned task's lifetime
//! to the guard.
//!
//! Wrapping the guard in a [`futures::future::Shared`] means the lifetime is
//! transitively tied to the last strong `Shared` clone. When all
//! subscribers drop, the guard drops, the task aborts.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::task::{JoinError, JoinHandle};

/// Wraps a [`JoinHandle`] so that dropping the wrapper aborts the spawned
/// task.
pub(crate) struct AbortOnDrop<V>(pub JoinHandle<V>);

impl<V> Drop for AbortOnDrop<V> {
    fn drop(&mut self) {
        // Aborting an already-completed task is a no-op.
        self.0.abort();
    }
}

impl<V: 'static> Future for AbortOnDrop<V> {
    type Output = Result<V, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // `JoinHandle` is `Unpin`, so we can freely re-pin it from `&mut`.
        Pin::new(&mut self.0).poll(cx)
    }
}
