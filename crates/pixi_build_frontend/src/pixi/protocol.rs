use crate::tool::Tool;
use crate::{CondaMetadata, CondaMetadataRequest};
use rattler_conda_types::ChannelConfig;

pub struct Protocol {
    pub(super) channel_config: ChannelConfig,
}

impl Protocol {
    /// Extract metadata from the recipe.
    pub fn get_conda_metadata(
        &self,
        _request: &CondaMetadataRequest,
    ) -> miette::Result<CondaMetadata> {
        todo!("extract metadata from pixi manifest")
    }
}
