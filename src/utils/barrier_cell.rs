use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, Ordering},
};
use thiserror::Error;
use tokio::sync::Notify;

pub struct BarrierCell<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
    notify: Notify,
}

unsafe impl<T: Sync> Sync for BarrierCell<T> {}
unsafe impl<T: Send> Send for BarrierCell<T> {}

#[repr(u8)]
enum BarrierCellState {
    Uninitialized,
    Initializing,
    Initialized,
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
            state: AtomicU8::new(BarrierCellState::Uninitialized as u8),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            notify: Notify::new(),
        }
    }

    /// Wait for a value to become available in the cell
    pub async fn wait(&self) -> &T {
        self.notify.notified().await;
        unsafe { (*self.value.get()).assume_init_ref() }
    }

    /// Wait for a value to become available in the cell or return a writer which
    pub fn set(&self, value: T) -> Result<(), SetError> {
        let state = self
            .state
            .fetch_max(BarrierCellState::Initializing as u8, Ordering::SeqCst);

        // If the state is larger than started writing, then either there is an active writer or
        // the cell has already been initialized.
        if state == BarrierCellState::Initialized as u8 {
            return Err(SetError::AlreadySet);
        } else {
            unsafe { *self.value.get() = MaybeUninit::new(value) };
            self.state
                .store(BarrierCellState::Initialized as u8, Ordering::Release);
            self.notify.notify_waiters();
        }

        Ok(())
    }

    pub fn into_inner(self) -> Option<T> {
        if self.state.load(Ordering::Acquire) == BarrierCellState::Initialized as u8 {
            Some(unsafe { self.value.into_inner().assume_init() })
        } else {
            None
        }
    }
}
