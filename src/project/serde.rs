use rattler_conda_types::{Channel, ChannelConfig};
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

pub struct ChannelStr;

impl<'de> serde_with::DeserializeAs<'de, Channel> for ChannelStr {
    fn deserialize_as<D>(deserializer: D) -> Result<Channel, D::Error>
    where
        D: Deserializer<'de>,
    {
        let channel_str = Cow::<str>::deserialize(deserializer)?;
        let channel_config = ChannelConfig::default();
        Channel::from_str(channel_str, &channel_config).map_err(D::Error::custom)
    }
}
