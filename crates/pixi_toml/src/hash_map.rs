use std::{
    collections::HashMap,
    hash::{BuildHasher, Hash, RandomState},
    str::FromStr,
};

use toml_span::{de_helpers::expected, value::ValueInner, DeserError, ErrorKind, Value};

use crate::{DeserializeAs, FromKey, Same};

/// [`HashMap`] is not supported by `toml_span` so we need to implement our own
/// deserializer.
///
/// The deserializer will expect a table and will attempt to deserialize the
/// keys and values from the document. The order is not retained.
pub struct TomlHashMap<K, V, H = RandomState>(HashMap<K, V, H>);

impl<K, V, H> TomlHashMap<K, V, H> {
    pub fn into_inner(self) -> HashMap<K, V, H> {
        self.0
    }
}

impl<K, V, H> From<TomlHashMap<K, V, H>> for HashMap<K, V, H> {
    fn from(value: TomlHashMap<K, V, H>) -> Self {
        value.0
    }
}

impl<'de, K: FromKey<'de> + Hash + Eq, T, U, H> DeserializeAs<'de, HashMap<K, T, H>>
    for TomlHashMap<K, U, H>
where
    <K as FromKey<'de>>::Err: std::fmt::Display,
    U: DeserializeAs<'de, T>,
    H: Default + BuildHasher,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<HashMap<K, T, H>, DeserError> {
        let table = match value.take() {
            ValueInner::Table(table) => table,
            other => {
                return Err(DeserError::from(expected("a table", other, value.span)));
            }
        };

        let mut errors = DeserError { errors: Vec::new() };
        let mut result = HashMap::default();
        for (key, mut value) in table.into_iter() {
            let key_span = key.span;
            let key = K::from_key(key).map_err(|e: <K as FromKey>::Err| toml_span::Error {
                kind: ErrorKind::Custom(e.to_string().into()),
                span: key_span,
                line_info: None,
            });

            let value = U::deserialize_as(&mut value);

            match (key, value) {
                (Ok(k), Ok(v)) => {
                    result.insert(k, v);
                }
                (Err(ke), Err(ve)) => {
                    errors.errors.push(ke);
                    errors.merge(ve);
                }
                (Err(e), _) => {
                    errors.errors.push(e);
                }
                (_, Err(e)) => {
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
}

impl<'de, K: FromStr + Hash + Eq, V: toml_span::Deserialize<'de>, H> toml_span::Deserialize<'de>
    for TomlHashMap<K, V, H>
where
    <K as FromKey<'de>>::Err: std::fmt::Display,
    H: Default + BuildHasher,
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        Ok(Self(TomlHashMap::<K, Same, H>::deserialize_as(value)?))
    }
}
