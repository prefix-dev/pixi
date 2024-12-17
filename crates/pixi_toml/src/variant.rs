use std::{fmt::Display, str::FromStr};

use toml_span::{DeserError, Error, ErrorKind, Value};

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
