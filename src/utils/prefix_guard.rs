use std::{
    io,
    io::{Read, Seek, Write},
    path::Path,
};

use fd_lock::RwLockWriteGuard;
use serde::{Deserialize, Serialize};

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

pub struct WriteGuard<'a> {
    guard: RwLockWriteGuard<'a, std::fs::File>,
    state: GuardState,
}

impl<'a> WriteGuard<'a> {
    fn new(mut guard: RwLockWriteGuard<'a, std::fs::File>) -> io::Result<Self> {
        let mut bytes = Vec::new();
        guard.read_to_end(&mut bytes)?;
        let state = serde_json::from_slice(&bytes).unwrap_or(GuardState::Unknown);
        Ok(Self { guard, state })
    }

    /// Returns true if the prefix is in a usable state.
    pub fn is_ready(&self) -> bool {
        self.state == GuardState::Ready
    }

    /// Notify this instance that installation of the prefix has started.
    pub fn begin(&mut self) -> io::Result<()> {
        if self.state != GuardState::Installing {
            self.guard.rewind()?;
            let bytes = serde_json::to_vec(&GuardState::Installing)?;
            self.guard.write_all(&bytes)?;
            self.guard.set_len(bytes.len() as u64)?;
            self.guard.flush()?;
            self.state = GuardState::Installing;
        }
        Ok(())
    }

    /// Finishes writing to the guard and releases the lock.
    pub fn finish(self) -> io::Result<()> {
        let WriteGuard {
            mut guard,
            state: status,
        } = self;
        if status == GuardState::Installing {
            guard.rewind()?;
            let bytes = serde_json::to_vec(&GuardState::Ready)?;
            guard.write_all(&bytes)?;
            guard.set_len(bytes.len() as u64)?;
            guard.flush()?;
        }
        Ok(())
    }
}

pub struct PrefixGuard {
    guard: fd_lock::RwLock<std::fs::File>,
}

impl PrefixGuard {
    /// Constructs a new guard for the given prefix but does not perform any
    /// locking operations yet.
    pub fn new(prefix: &Path) -> io::Result<Self> {
        let guard_path = prefix.join(GUARD_PATH);

        // Ensure that the directory exists
        std::fs::create_dir_all(guard_path.parent().unwrap())?;

        // Open the file
        Ok(Self {
            guard: fd_lock::RwLock::new(
                std::fs::File::options()
                    .write(true)
                    .read(true)
                    .create(true)
                    .truncate(false)
                    .open(guard_path)?,
            ),
        })
    }

    /// Locks the guard for writing and returns a write guard which can be used
    /// to unlock it.
    pub fn write(&mut self) -> io::Result<WriteGuard> {
        WriteGuard::new(self.guard.write()?)
    }
}
