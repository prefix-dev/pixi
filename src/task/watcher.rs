use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use notify::{event::ModifyKind, EventKind, RecursiveMode};
use notify_debouncer_full::new_debouncer;
use thiserror::Error;
use tokio::sync::mpsc::{self, Receiver};
use tracing::{error, info};
use wax::Glob;

/// Errors that can occur when watching files.
#[derive(Debug, Error)]
pub enum FileWatchError {
    /// An error occurred while watching files.
    #[error("Error watching files")]
    WatchError,

    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Notify error.
    #[error("Notify error: {0}")]
    NotifyError(#[from] notify::Error),

    /// Pattern error.
    #[error("Pattern error: {0}")]
    PatternError(#[from] wax::BuildError),
}

/// Represents a file change event
#[derive(Debug, Clone)]
pub struct FileEvent {
    /// Paths that were changed
    pub paths: Vec<PathBuf>,
}

/// Watches files for changes
pub struct FileWatcher {
    rx: Receiver<Result<FileEvent, FileWatchError>>,
    // Keep the debouncer alive
    _debouncer: Box<dyn std::any::Any + Send>,
}

impl FileWatcher {
    /// Creates a new file watcher
    pub async fn new(
        cwd: &Path,
        patterns: &[impl AsRef<Path>],
        debounce: Duration,
    ) -> Result<Self, FileWatchError> {
        let (tx, rx) = mpsc::channel(100);

        // Create debouncer based on example
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();
        let mut debouncer =
            new_debouncer(debounce, None, notify_tx).map_err(|_| FileWatchError::WatchError)?;

        // Process all input paths
        for pattern in patterns {
            let path = pattern.as_ref();
            let path_str = path.to_string_lossy();

            info!("Processing glob pattern: {}", path_str);

            // Compile the glob pattern
            let glob = Glob::new(&path_str)?;

            // Try to find existing files matching the pattern
            for entry in glob.walk(cwd).flatten() {
                let path = entry.path().to_path_buf();
                // Convert to absolute path if it's relative
                let path = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };

                debouncer.watch(&path, RecursiveMode::Recursive)?;
            }
        }

        // Handle events from notify
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            for result in notify_rx {
                match result {
                    Ok(events) => {
                        let events = events
                            .into_iter()
                            .filter(|event| match event.event.kind {
                                EventKind::Modify(kind) => !matches!(kind, ModifyKind::Metadata(_)),
                                EventKind::Create(_) | EventKind::Remove(_) => true,
                                _ => false,
                            })
                            .collect::<Vec<_>>();
                        // Extract all paths from the events
                        let filtered_paths: Vec<PathBuf> = events
                            .into_iter()
                            .flat_map(|event| event.event.paths)
                            .collect();

                        // Deduplicate paths
                        let mut unique_paths = std::collections::HashSet::new();
                        let deduplicated_paths: Vec<PathBuf> = filtered_paths
                            .into_iter()
                            .filter(|path| unique_paths.insert(path.clone()))
                            .collect();

                        if !deduplicated_paths.is_empty() {
                            if let Err(e) = tx_clone
                                .send(Ok(FileEvent {
                                    paths: deduplicated_paths,
                                }))
                                .await
                            {
                                error!("Failed to send file event: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("File watch error: {:?}", e);
                        if let Err(e) = tx_clone.send(Err(FileWatchError::WatchError)).await {
                            error!("Failed to send error event: {}", e);
                        }
                    }
                }
            }
        });

        Ok(Self {
            rx,
            _debouncer: Box::new(debouncer),
        })
    }

    /// Returns the next file change event
    pub async fn next(&mut self) -> Option<Result<FileEvent, FileWatchError>> {
        self.rx.recv().await
    }
}
