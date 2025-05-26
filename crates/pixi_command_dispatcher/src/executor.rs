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

pub(crate) enum ExecutorFutures<Fut> {
    Concurrent(FuturesUnordered<Fut>),
    Serial(Vec<Fut>),
}

impl<Fut> ExecutorFutures<Fut> {
    pub fn new(executor: Executor) -> Self {
        match executor {
            Executor::Concurrent => Self::Concurrent(FuturesUnordered::new()),
            Executor::Serial => Self::Serial(Vec::new()),
        }
    }

    pub fn push(&mut self, fut: Fut) {
        match self {
            ExecutorFutures::Concurrent(futures) => futures.push(fut),
            ExecutorFutures::Serial(futures) => futures.push(fut),
        }
    }
}

impl<Fut> Stream for ExecutorFutures<Fut>
where
    Fut: Future + Unpin,
{
    type Item = Fut::Output;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this {
            ExecutorFutures::Concurrent(futures) => Pin::new(futures).poll_next(cx),
            ExecutorFutures::Serial(futures) => {
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
            ExecutorFutures::Concurrent(futures) => futures.size_hint(),
            ExecutorFutures::Serial(futures) => (futures.len(), Some(futures.len())),
        }
    }
}
