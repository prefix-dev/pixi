use digest::{Digest, Output};
use toml_span::{DeserError, Deserialize, ErrorKind, Value};

use crate::DeserializeAs;

/// Parse a digest from a string TOML value.
pub struct TomlDigest<D: Digest>(Output<D>);

impl<D: Digest> TomlDigest<D> {
    pub fn into_inner(self) -> Output<D> {
        self.0
    }
}

impl<'de, D: Digest> toml_span::Deserialize<'de> for TomlDigest<D> {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let value_str = value.take_string(None)?;
        let mut hash = <Output<D>>::default();
        match hex::decode_to_slice(value_str.as_ref(), &mut hash) {
            Ok(_) => Ok(TomlDigest(hash)),
            Err(e) => Err(toml_span::Error {
                kind: ErrorKind::Custom(e.to_string().into()),
                span: value.span,
                line_info: None,
            }
            .into()),
        }
    }
}

impl<'de, D: Digest> DeserializeAs<'de, Output<D>> for TomlDigest<D> {
    fn deserialize_as(value: &mut Value<'de>) -> Result<Output<D>, DeserError> {
        Self::deserialize(value).map(|digest| digest.into_inner())
    }
}
