use std::{fmt::Display, str::FromStr};

use toml_span::{DeserError, Error, ErrorKind, Value};

/// A wrapper around an enum that implements `FromStr` and
/// [`strum::VariantNames`] for deserialization.
///
/// This type will parse the type as a string and then attempt to parse it. If
/// parsing fails a [`ErrorKind::UnexpectedValue`] error will be returned which
/// contains the possible values of the enum.
pub struct TomlEnum<T>(T);

impl<T> TomlEnum<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<'de, T: strum::VariantNames + FromStr + Display> toml_span::Deserialize<'de> for TomlEnum<T>
where
    <T as FromStr>::Err: std::fmt::Display,
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let value_str = value.take_string(None)?;
        value_str.parse().map(TomlEnum).map_err(|_| {
            DeserError::from(Error {
                kind: ErrorKind::UnexpectedValue {
                    expected: T::VARIANTS,
                    value: Some(value_str.to_string()),
                },
                span: value.span,
                line_info: None,
            })
        })
    }
}
