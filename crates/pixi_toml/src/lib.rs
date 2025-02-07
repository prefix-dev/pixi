//! This crate provides utilities to work with [`toml_span`].
//!
//! Deserialization helper structs are provide to deserialize types that are not
//! directly supported by [`toml_span`].
//!
//! This crate also provides [`TomlWith`] which enables directly parsing from
//! one type into another. This makes it possible to efficiently implement
//! deserializers for types from external crates.

mod digest;
mod from_str;
mod hash_map;
mod index_map;
mod index_set;
mod one_or_many;
mod variant;
mod with;

use std::str::FromStr;

pub use digest::TomlDigest;
pub use from_str::TomlFromStr;
pub use hash_map::TomlHashMap;
pub use index_map::TomlIndexMap;
pub use index_set::TomlIndexSet;
pub use one_or_many::OneOrMany;
use toml_span::{de_helpers::expected, value::ValueInner, DeserError, Spanned, Value};
pub use variant::TomlEnum;
pub use with::TomlWith;

/// A trait that enables efficient deserialization of one type into another.
pub trait DeserializeAs<'de, T> {
    fn deserialize_as(value: &mut Value<'de>) -> Result<T, DeserError>;
}

pub struct Same;

impl<'de, T: toml_span::Deserialize<'de>> DeserializeAs<'de, T> for Same {
    fn deserialize_as(value: &mut Value<'de>) -> Result<T, DeserError> {
        T::deserialize(value)
    }
}

impl<'de, T, U: DeserializeAs<'de, T>> DeserializeAs<'de, Spanned<T>> for Spanned<U> {
    fn deserialize_as(value: &mut Value<'de>) -> Result<Spanned<T>, DeserError> {
        let span = value.span;
        let value = U::deserialize_as(value)?;
        Ok(Spanned { value, span })
    }
}

impl<'de, T, U> DeserializeAs<'de, Vec<T>> for Vec<U>
where
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<Vec<T>, DeserError> {
        let array = match value.take() {
            ValueInner::Array(array) => array,
            other => {
                return Err(DeserError::from(expected("array", other, value.span)));
            }
        };

        let mut errors = DeserError { errors: Vec::new() };
        let mut result = Vec::with_capacity(array.len());
        for mut value in array {
            match U::deserialize_as(&mut value) {
                Ok(v) => result.push(v),
                Err(e) => errors.merge(e),
            }
        }

        if errors.errors.is_empty() {
            Ok(result)
        } else {
            Err(errors)
        }
    }
}

pub trait FromKey<'de>: Sized {
    type Err;

    fn from_key(key: toml_span::value::Key<'de>) -> Result<Self, Self::Err>;
}

impl<'de, T: FromStr> FromKey<'de> for T {
    type Err = <T as FromStr>::Err;

    fn from_key(key: toml_span::value::Key<'de>) -> Result<Self, Self::Err> {
        key.name.parse()
    }
}
