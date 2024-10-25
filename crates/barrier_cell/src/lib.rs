use std::sync::OnceLock;
use thiserror::Error;
use tokio::sync::Notify;

/// A synchronization primitive, around [`OnceLock`] that can be used to wait for a value to become available.
///
/// The [`BarrierCell`] is initially empty, requesters can wait for a value to become available
/// using the `wait` method. Once a value is available, the `set` method can be used to set the
/// value in the cell. The `set` method can only be called once. If the `set` method is called
/// multiple times, it will return an error. When `set` is called successfully all waiters will be notified.
pub struct BarrierCell<T> {
    value: OnceLock<T>,
    notify: Notify,
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
        Self {
            value: OnceLock::new(),
            notify: Notify::new(),
        }
    }

    /// Wait for a value to become available in the cell
    pub async fn wait(&self) -> &T {
        self.notify.notified().await;
        // safe, as notification only occurs after setting the value
        unsafe { self.value.get().unwrap_unchecked() }
    }

    /// Set the value in the cell, if the cell was already initialized this will return an error.
    pub fn set(&self, value: T) -> Result<(), SetError> {
        self.value.set(value).map_err(|_| SetError::AlreadySet)?;
        self.notify.notify_waiters();

        Ok(())
    }

    /// Consumes this instance and converts it into the inner value if it has been initialized.
    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }
}
