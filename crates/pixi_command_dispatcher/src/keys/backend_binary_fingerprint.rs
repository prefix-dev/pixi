//! Compute-engine Key that fingerprints a backend executable on disk.
//!
//! Used by system / path-based backend overrides as a content-derived
//! identifier so their cached metadata stays valid as long as the binary
//! hasn't changed. The engine caches the value within a process; on disk
//! the fingerprint is stored in
//! [`crate::cache::backend_metadata::BuildBackendMetadataCacheEntry`] and
//! compared on cache probe.

use std::{
    fs::File,
    io::{BufReader, Read},
    path::PathBuf,
    sync::Arc,
};

use derive_more::Display;
use pixi_compute_engine::{ComputeCtx, Key};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

use crate::input_hash::BackendBinaryFingerprint;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct BackendBinaryFingerprintSpec {
    pub path: PathBuf,
}

#[derive(Clone, Debug, Display, Eq, Hash, PartialEq)]
#[display("{}", _0.path.display())]
pub struct BackendBinaryFingerprintKey(pub Arc<BackendBinaryFingerprintSpec>);

impl BackendBinaryFingerprintKey {
    pub fn new(path: PathBuf) -> Self {
        Self(Arc::new(BackendBinaryFingerprintSpec { path }))
    }
}

impl Key for BackendBinaryFingerprintKey {
    type Value = Result<BackendBinaryFingerprint, BackendBinaryFingerprintError>;

    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        let path = self.0.path.clone();
        tokio::task::spawn_blocking(move || {
            let file = File::open(&path)
                .map_err(|err| BackendBinaryFingerprintError::new(path.clone(), err))?;
            let mut reader = BufReader::new(file);
            let mut hasher = Xxh3::new();
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader
                    .read(&mut buf)
                    .map_err(|err| BackendBinaryFingerprintError::new(path.clone(), err))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            Ok(BackendBinaryFingerprint::new(hasher.digest()))
        })
        .await
        .expect("spawn_blocking panicked while fingerprinting backend binary")
    }
}

#[derive(Debug, Clone, Error)]
#[error("failed to fingerprint backend binary at {path}")]
pub struct BackendBinaryFingerprintError {
    path: PathBuf,
    #[source]
    source: Arc<std::io::Error>,
}

impl BackendBinaryFingerprintError {
    fn new(path: PathBuf, source: std::io::Error) -> Self {
        Self {
            path,
            source: Arc::new(source),
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}
