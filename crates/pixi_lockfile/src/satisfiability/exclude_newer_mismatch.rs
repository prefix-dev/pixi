use std::fmt::{Display, Formatter};

use thiserror::Error;

#[derive(Debug, Error)]
pub struct ExcludeNewerMismatch {
    pub locked_exclude_newer: Option<chrono::DateTime<chrono::Utc>>,
    pub expected_exclude_newer: Option<chrono::DateTime<chrono::Utc>>,
}

impl Display for ExcludeNewerMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (self.locked_exclude_newer, self.expected_exclude_newer) {
            (Some(locked), None) => {
                write!(
                    f,
                    "the lock-file was solved with exclude-newer set to {locked}, but the environment does not have this option set"
                )
            }
            (None, Some(expected)) => {
                write!(
                    f,
                    "the lock-file was solved without exclude-newer, but the environment has this option set to {expected}"
                )
            }
            (Some(locked), Some(expected)) if locked != expected => {
                write!(
                    f,
                    "the lock-file was solved with exclude-newer set to {locked}, but the environment has this option set to {expected}"
                )
            }
            _ => unreachable!("if we get here the values are the same"),
        }
    }
}
