use std::collections::BTreeSet;

use toml_span::{DeserError, Value, de_helpers::expected, value::ValueInner};

use crate::DeserializeAs;

/// [`BTreeSet`] is not supported by `toml_span` directly so we provide our own
/// deserializer.
///
/// The deserializer expects an array and deduplicates the entries.
pub struct TomlBTreeSet<T>(BTreeSet<T>);

impl<T> TomlBTreeSet<T> {
    pub fn into_inner(self) -> BTreeSet<T> {
        self.0
    }
}

impl<T> From<TomlBTreeSet<T>> for BTreeSet<T> {
    fn from(value: TomlBTreeSet<T>) -> Self {
        value.0
    }
}

impl<'de, T: toml_span::Deserialize<'de> + Ord> toml_span::Deserialize<'de> for TomlBTreeSet<T> {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::Array(array) => {
                let mut errors = DeserError { errors: Vec::new() };
                let mut result = BTreeSet::new();
                for mut value in array {
                    match T::deserialize(&mut value) {
                        Ok(v) => {
                            result.insert(v);
                        }
                        Err(e) => {
                            errors.merge(e);
                        }
                    }
                }
                if errors.errors.is_empty() {
                    Ok(Self(result))
                } else {
                    Err(errors)
                }
            }
            other => Err(DeserError::from(expected("array", other, value.span))),
        }
    }
}

impl<'de, T: Ord, U> DeserializeAs<'de, BTreeSet<T>> for TomlBTreeSet<U>
where
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<BTreeSet<T>, DeserError> {
        match value.take() {
            ValueInner::Array(array) => {
                let mut errors = DeserError { errors: Vec::new() };
                let mut result = BTreeSet::new();
                for mut value in array {
                    match U::deserialize_as(&mut value) {
                        Ok(v) => {
                            result.insert(v);
                        }
                        Err(e) => {
                            errors.merge(e);
                        }
                    }
                }
                if errors.errors.is_empty() {
                    Ok(result)
                } else {
                    Err(errors)
                }
            }
            other => Err(DeserError::from(expected("array", other, value.span))),
        }
    }
}
