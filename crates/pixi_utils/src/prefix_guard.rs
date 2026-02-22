use std::{io, path::Path};

use async_fd_lock::LockWrite;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::{self, io::AsyncSeekExt};

const GUARD_PATH: &str = ".guard";

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GuardState {
    /// Unknown
    Unknown,

    /// The prefix is currently being installed.
    Installing,

    /// The prefix has been successfully installed and is ready to be used.
    Ready,
}

#[derive(Debug)]
pub struct AsyncWriteGuard {
    guard: async_fd_lock::RwLockWriteGuard<tokio::fs::File>,
    state: GuardState,
}

impl AsyncWriteGuard {
    async fn new(mut guard: async_fd_lock::RwLockWriteGuard<tokio::fs::File>) -> io::Result<Self> {
        let mut bytes = Vec::new();
        guard.read_to_end(&mut bytes).await?;
        let state = serde_json::from_slice(&bytes).unwrap_or(GuardState::Unknown);
        Ok(Self { guard, state })
    }

    /// Returns true if the prefix is in a usable state.
    pub fn is_ready(&self) -> bool {
        self.state == GuardState::Ready
    }

    /// Notify this instance that installation of the prefix has started.
    pub async fn begin(&mut self) -> io::Result<()> {
        if self.state != GuardState::Installing {
            self.guard.rewind().await?;
            let bytes = serde_json::to_vec(&GuardState::Installing)?;
            self.guard.write_all(&bytes).await?;
            // self.guard.set_len(bytes.len() as u64)?;
            self.guard.flush().await?;
            self.state = GuardState::Installing;
        }
        Ok(())
    }

    /// Finishes writing to the guard and releases the lock.
    pub async fn finish(self) -> io::Result<()> {
        let AsyncWriteGuard {
            mut guard,
            state: status,
        } = self;
        if status == GuardState::Installing {
            guard.rewind().await?;
            let bytes = serde_json::to_vec(&GuardState::Ready)?;
            guard.write_all(&bytes).await?;
            guard.flush().await?;
        }
        Ok(())
    }
}

pub struct AsyncPrefixGuard {
    guard: tokio::fs::File,
}

impl AsyncPrefixGuard {
    /// Constructs a new guard for the given prefix but does not perform any
    /// locking operations yet.
    pub async fn new(prefix: &Path) -> io::Result<Self> {
        let guard_path = prefix.join(GUARD_PATH);

        // Ensure that the directory exists
        fs_err::tokio::create_dir_all(guard_path.parent().unwrap()).await?;

        let file = tokio::fs::File::options()
            .write(true)
            .read(true)
            .create(true)
            .truncate(false)
            .open(guard_path)
            .await?;

        // Open the file
        Ok(Self { guard: file })
    }

    /// Locks the guard for writing and returns a write guard which can be used
    /// to unlock it.
    pub async fn write(self) -> io::Result<AsyncWriteGuard> {
        let write_guard = self.guard.lock_write().await?;

        AsyncWriteGuard::new(write_guard).await
    }
}
