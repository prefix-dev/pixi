use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, stream::FuturesUnordered};

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
