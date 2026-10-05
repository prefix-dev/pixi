use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

/// Whether this compute session may execute a build backend, check out a
/// source repository, or build a PyPI source distribution.
///
/// Stored in the [`crate::DataStore`] so every compute task sees the same
/// decision. A missing entry is not permission to spawn: readers must treat
/// absence as deny.
#[derive(Debug, Clone)]
pub struct BuildExecutionPermit {
    allowed: Arc<AtomicBool>,
}

/// Returned when a denied permit blocks a build-backend spawn or source checkout.
#[derive(Debug, Clone, Error)]
#[error("refusing to invoke a build backend")]
pub struct BuildExecutionDenied;

impl BuildExecutionPermit {
    /// Permit build-backend execution. Callers that are not `pixi lock`,
    /// `pixi update`, or `pixi upgrade --no-build` pass this explicitly.
    pub fn allow() -> Self {
        Self {
            allowed: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Refuse every build-backend spawn, in-process backend, and source checkout.
    pub fn deny() -> Self {
        Self {
            allowed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether execution is permitted.
    pub fn is_allowed(&self) -> bool {
        self.allowed.load(Ordering::Acquire)
    }

    /// Fail if this permit denies execution.
    pub fn check(&self) -> Result<(), BuildExecutionDenied> {
        if self.is_allowed() {
            Ok(())
        } else {
            Err(BuildExecutionDenied)
        }
    }
}

impl Default for BuildExecutionPermit {
    fn default() -> Self {
        Self::allow()
    }
}
