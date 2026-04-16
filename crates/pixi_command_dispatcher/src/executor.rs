use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, stream::FuturesUnordered};

use crate::CommandDispatcherError;

/// Defines the executor to use in the command dispatcher background task.
///
/// By default, the command dispatcher will use [`Self::Concurrent`] which will
/// run all futures concurrently. However, the [`Self::Serial`] executor
/// can be used to run futures in a deterministic order. This is useful for
/// testing and debugging purposes.
#[derive(Debug, Clone, Copy, Default)]
pub enum Executor {
    /// Runs all futures concurrently. This is the default executor.
    #[default]
    Concurrent,
    /// Deterministically polls futures in LIFO order. This is useful for
    /// testing purposes.
    Serial,
}

pin_project_lite::pin_project! {
    /// A collection of futures that can be executed either concurrently or serially.
    ///
    /// This type provides a unified interface for managing multiple futures with different
    /// execution strategies. The execution mode is determined by the [`Executor`] passed
    /// to [`ExecutorFutures::new`].
    ///
    /// # Usage
    ///
    /// Typically, you should obtain the executor from [`CommandDispatcher::executor()`]
    /// rather than hardcoding a specific executor:
    ///
    /// ```ignore
    /// // Get executor from the command dispatcher
    /// let mut futures = ExecutorFutures::new(command_dispatcher.executor());
    ///
    /// // Push futures into the collection
    /// for item in items {
    ///     futures.push(process_item(item));
    /// }
    ///
    /// // Collect results as they complete
    /// while let Some(result) = futures.next().await {
    ///     // Handle result
    /// }
    /// ```
    ///
    /// This ensures that:
    /// - Production code uses concurrent execution for better performance
    /// - Tests can use serial execution for deterministic behavior
    /// - The execution mode is configured in one place (the dispatcher builder)
    #[project = ExecutorFuturesProj]
    pub(crate) enum ExecutorFutures<Fut> {
        Concurrent { #[pin] futures: FuturesUnordered<Fut> },
        Serial { futures: Vec<Pin<Box<Fut>>> },
    }
}

impl<Fut> ExecutorFutures<Fut> {
    /// Creates a new `ExecutorFutures` with the specified execution strategy.
    ///
    /// # Recommendation
    ///
    /// Instead of hardcoding `Executor::Concurrent` or `Executor::Serial`, prefer
    /// obtaining the executor from [`CommandDispatcher::executor()`]:
    ///
    /// ```ignore
    /// let mut futures = ExecutorFutures::new(command_dispatcher.executor());
    /// ```
    pub fn new(executor: Executor) -> Self {
        match executor {
            Executor::Concurrent => Self::Concurrent {
                futures: FuturesUnordered::new(),
            },
            Executor::Serial => Self::Serial {
                futures: Vec::new(),
            },
        }
    }

    /// Adds a future to the collection.
    ///
    /// The future will be executed according to the execution strategy:
    /// - `Concurrent`: The future may be polled in any order with other futures
    /// - `Serial`: The future will be polled in LIFO (last-in-first-out) order
    pub fn push(&mut self, fut: Fut) {
        match self {
            ExecutorFutures::Concurrent { futures } => futures.push(fut),
            ExecutorFutures::Serial { futures } => futures.push(Box::pin(fut)),
        }
    }

    /// Returns the number of futures in the collection.
    pub fn len(&self) -> usize {
        match self {
            ExecutorFutures::Concurrent { futures } => futures.len(),
            ExecutorFutures::Serial { futures } => futures.len(),
        }
    }
}

impl<Fut> Stream for ExecutorFutures<Fut>
where
    Fut: Future,
{
    type Item = Fut::Output;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.project() {
            ExecutorFuturesProj::Concurrent { futures } => futures.poll_next(cx),
            ExecutorFuturesProj::Serial { futures } => {
                if let Some(mut fut) = futures.last_mut() {
                    match Pin::new(&mut fut).poll(cx) {
                        Poll::Ready(result) => {
                            futures.pop();
                            Poll::Ready(Some(result))
                        }
                        Poll::Pending => Poll::Pending,
                    }
                } else {
                    Poll::Ready(None)
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            ExecutorFutures::Concurrent { futures } => futures.size_hint(),
            ExecutorFutures::Serial { futures } => (futures.len(), Some(futures.len())),
        }
    }
}

/// A collection of cancellation-aware futures.
///
/// When any future yields `Err(CommandDispatcherError::Cancelled)`, the
/// collection marks itself as cancelled. New futures pushed after that point
/// are immediately dropped without being polled.
///
/// The [`Stream`] implementation filters out `Cancelled` results, yielding only
/// real successes and failures. When the underlying stream ends and any
/// cancellation was observed, a single final `Cancelled` sentinel is emitted.
pub struct CancellationAwareFutures<Fut> {
    inner: ExecutorFutures<Fut>,
    any_cancelled: bool,
    done: bool,
}

impl<Fut> CancellationAwareFutures<Fut> {
    /// Creates a new `CancellationAwareFutures` with the specified execution
    /// strategy.
    pub fn new(executor: Executor) -> Self {
        Self {
            inner: ExecutorFutures::new(executor),
            any_cancelled: false,
            done: false,
        }
    }

    /// Adds a future to the collection, or drops it immediately if
    /// cancellation has already been observed.
    pub fn push(&mut self, fut: Fut) {
        if !self.any_cancelled {
            self.inner.push(fut);
        }
    }

    /// Returns whether any cancellation was observed.
    pub fn is_cancelled(&self) -> bool {
        self.any_cancelled
    }

    /// Returns the number of futures in the collection.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the collection contains no futures.
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }
}

impl<Fut, T, E> CancellationAwareFutures<Fut>
where
    Fut: Future<Output = Result<T, CommandDispatcherError<E>>>,
{
    /// Drain all futures, collecting successes and errors separately.
    ///
    /// Cancelled results are filtered out. Returns `Err(Cancelled)` only
    /// if ALL futures were cancelled with no real results.
    pub async fn collect_all(&mut self) -> Result<(Vec<T>, Vec<E>), CommandDispatcherError<E>> {
        use futures::StreamExt;

        let mut successes = Vec::new();
        let mut errors = Vec::new();
        while let Some(result) = self.next().await {
            match result {
                Ok(value) => successes.push(value),
                Err(CommandDispatcherError::Failed(err)) => errors.push(err),
                Err(CommandDispatcherError::Cancelled) => {}
            }
        }
        if successes.is_empty() && errors.is_empty() {
            Err(CommandDispatcherError::Cancelled)
        } else {
            Ok((successes, errors))
        }
    }
}

impl<Fut, T, E> Stream for CancellationAwareFutures<Fut>
where
    Fut: Future<Output = Result<T, CommandDispatcherError<E>>>,
{
    type Item = Result<T, CommandDispatcherError<E>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Err(CommandDispatcherError::Cancelled))) => {
                    this.any_cancelled = true;
                    continue;
                }
                Poll::Ready(Some(item)) => return Poll::Ready(Some(item)),
                Poll::Ready(None) => {
                    this.done = true;
                    if this.any_cancelled {
                        return Poll::Ready(Some(Err(CommandDispatcherError::Cancelled)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
