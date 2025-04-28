use std::hash::Hash;

use indexmap::IndexSet;
use itertools::Itertools;
use toml_span::{DeserError, Value, de_helpers::expected, value::ValueInner};

use crate::DeserializeAs;

/// [`IndexSet`] is not supported by `toml_span` so we need to implement our own
/// deserializer.
///
/// The deserializer will expect a table and will attempt to deserialize the
/// values in the order they are defined in the document.
pub struct TomlIndexSet<T>(IndexSet<T>);

impl<T> TomlIndexSet<T> {
    pub fn into_inner(self) -> IndexSet<T> {
        self.0
    }
}

impl<T> From<TomlIndexSet<T>> for IndexSet<T> {
    fn from(value: TomlIndexSet<T>) -> Self {
        value.0
    }
}

impl<'de, T: toml_span::Deserialize<'de> + Hash + Eq> toml_span::Deserialize<'de>
    for TomlIndexSet<T>
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::Array(array) => {
                let mut errors = DeserError { errors: Vec::new() };
                let mut result = IndexSet::new();
                for mut value in array.into_iter().sorted_by_key(|value| value.span.start) {
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

impl<'de, T: Hash + Eq, U> DeserializeAs<'de, IndexSet<T>> for TomlIndexSet<U>
where
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<IndexSet<T>, DeserError> {
        match value.take() {
            ValueInner::Array(array) => {
                let mut errors = DeserError { errors: Vec::new() };
                let mut result = IndexSet::new();
                for mut value in array.into_iter().sorted_by_key(|value| value.span.start) {
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
