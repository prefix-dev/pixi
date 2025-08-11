use std::fmt::{Display, Formatter};

use rattler_lock::PackageHashes;
use thiserror::Error;

#[derive(Debug, Error)]
pub struct SourceTreeHashMismatch {
    pub computed: PackageHashes,
    pub locked: Option<PackageHashes>,
}

impl Display for SourceTreeHashMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let computed_hash = self
            .computed
            .sha256()
            .map(|hash| format!("{:x}", hash))
            .or(self.computed.md5().map(|hash| format!("{:x}", hash)));
        let locked_hash = self.locked.as_ref().and_then(|hash| {
            hash.sha256()
                .map(|hash| format!("{:x}", hash))
                .or(hash.md5().map(|hash| format!("{:x}", hash)))
        });

        match (computed_hash, locked_hash) {
            (None, None) => write!(f, "could not compute a source tree hash"),
            (Some(computed), None) => {
                write!(
                    f,
                    "the computed source tree hash is '{}', but the lock-file does not contain a hash",
                    computed
                )
            }
            (Some(computed), Some(locked)) => write!(
                f,
                "the computed source tree hash is '{}', but the lock-file contains '{}'",
                computed, locked
            ),
            (None, Some(locked)) => write!(
                f,
                "could not compute a source tree hash, but the lock-file contains '{}'",
                locked
            ),
        }
    }
}
