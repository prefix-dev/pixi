use std::fmt::Display;

use rattler_lock::PypiIndexes;
use thiserror::Error;

#[derive(Debug, Error)]
pub struct IndexesMismatch {
    pub current: PypiIndexes,
    pub previous: Option<PypiIndexes>,
}

impl Display for IndexesMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(previous) = &self.previous {
            write!(
                f,
                "the indexes used to previously solve to lock file do not match the environments indexes.\n \
                Expected: {expected:#?}\n Found: {found:#?}",
                expected = previous,
                found = self.current
            )
        } else {
            write!(
                f,
                "the indexes used to previously solve to lock file are missing"
            )
        }
    }
}
