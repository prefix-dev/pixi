use std::sync::Arc;

use parking_lot::Mutex;
use thiserror::Error;

/// A synchronization primitive, that can be used to wait for a value to become
/// available.
///
/// The [`BarrierCell`] is initially empty, requesters can wait for a value to
/// become available using the `wait` method. Once a value is available, the
/// `set` method can be used to set the value in the cell. The `set` method can
/// only be called once. If the `set` method is called multiple times, it will
/// return an error. When `set` is called successfully all waiters will be
/// notified.
pub struct BarrierCell<T> {
    data: Mutex<Option<ValueOrNotify<T>>>,
}

enum ValueOrNotify<T> {
    Value(Arc<T>),
    Notify(tokio::sync::broadcast::Sender<Arc<T>>),
}

impl<T> Default for BarrierCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Error)]
pub enum SetError {
    #[error("cannot assign a BarrierCell twice")]
    AlreadySet,
}

impl<T> BarrierCell<T> {
    /// Constructs a new instance.
    pub fn new() -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(1);
        Self {
            data: Mutex::new(Some(ValueOrNotify::Notify(sender))),
        }
    }

    /// Wait for a value to become available in the cell
    #[expect(clippy::await_holding_lock)]
    pub async fn wait(&self) -> Arc<T> {
        let lock = self.data.lock();
        match &*lock {
            Some(ValueOrNotify::Value(value)) => value.clone(),
            Some(ValueOrNotify::Notify(notify)) => {
                let mut recv = notify.subscribe();
                drop(lock);
                recv.recv().await.expect("notify channel closed")
            }
            _ => unreachable!(),
        }
    }

    /// Set the value in the cell, if the cell was already initialized this will
    /// return an error.
    pub fn set(&self, value: Arc<T>) -> Result<(), SetError> {
        let mut lock = self.data.lock();
        match lock.take() {
            Some(ValueOrNotify::Value(value)) => {
                *lock = Some(ValueOrNotify::Value(value));
                Err(SetError::AlreadySet)
            }
            Some(ValueOrNotify::Notify(notify)) => {
                *lock = Some(ValueOrNotify::Value(value.clone()));
                drop(lock);
                let _ = notify.send(value);
                Ok(())
            }
            _ => unreachable!(),
        }
    }

    /// Consumes this instance and converts it into the inner value if it has
    /// been initialized.
    pub fn into_inner(self) -> Option<Arc<T>> {
        self.data.into_inner().and_then(|v| match v {
            ValueOrNotify::Value(value) => Some(value),
            _ => None,
        })
    }
}
