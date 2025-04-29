use pixi_spec::SourceSpec;
use thiserror::Error;

use crate::{CommandQueue, CommandQueueError, SourceCheckoutError};

/// Represents a request for source metadata.
pub struct SourceMetadataSpec {
    /// The source specification
    pub source_spec: SourceSpec,
}

impl SourceMetadataSpec {
    pub(crate) async fn request(
        self,
        command_queue: CommandQueue,
    ) -> Result<(), CommandQueueError<SourceMetadataError>> {
        // Get the pinned source for this source spec.
        let source = command_queue
            .pin_and_checkout(self.source_spec.clone())
            .await
            .map_err(|err| err.map(SourceMetadataError::SourceCheckoutError))?;

        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum SourceMetadataError {
    #[error(transparent)]
    SourceCheckoutError(#[from] SourceCheckoutError),
}
