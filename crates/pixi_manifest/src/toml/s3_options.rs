use crate::S3Options;

use pixi_toml::TomlFromStr;
use toml_span::{de_helpers::TableHelper, DeserError, Value};

impl<'de> toml_span::Deserialize<'de> for S3Options {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let endpoint_url = th
            .required::<TomlFromStr<_>>("endpoint-url")
            .map(TomlFromStr::into_inner)?;
        let region = th.required("region")?;
        let force_path_style = th.required("force-path-style")?;
        th.finalize(None)?;

        Ok(Self {
            endpoint_url,
            region,
            force_path_style,
        })
    }
}
