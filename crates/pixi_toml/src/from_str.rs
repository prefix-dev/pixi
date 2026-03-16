use std::str::FromStr;

use toml_span::{DeserError, Deserialize, Error, ErrorKind, Value};

use crate::DeserializeAs;

/// A helper type that implements [`toml_span::Deserialize`] for types that
/// implement [`FromStr`].
///
/// It often happens that a certain type is encoded as a string and needs
/// additional parsing beyond checking if the value is a string. If a type
/// provides a [`FromStr`] implementation, this type can be used to deserialize
/// the value and return the parsed value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TomlFromStr<T>(T);

impl<T> TomlFromStr<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<'de, T> DeserializeAs<'de, T> for TomlFromStr<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<T, DeserError> {
        TomlFromStr::deserialize(value).map(TomlFromStr::into_inner)
    }
}

impl<'de, T> Deserialize<'de> for TomlFromStr<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let span = value.span;
        value
            .take_string("expected a string".into())?
            .parse()
            .map_err(|e: <T as FromStr>::Err| {
                DeserError::from(Error {
                    kind: ErrorKind::Custom(e.to_string().into()),
                    span,
                    line_info: None,
                })
            })
            .map(Self)
    }
}
