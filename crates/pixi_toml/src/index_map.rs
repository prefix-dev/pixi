use std::hash::Hash;

use indexmap::IndexMap;
use itertools::Itertools;
use toml_span::{DeserError, Error, ErrorKind, Value, de_helpers::expected, value::ValueInner};

use crate::{DeserializeAs, FromKey, Same};

/// [`IndexMap`] is not supported by `toml_span` so we need to implement our own
/// deserializer.
///
/// The deserializer will expect a table and will attempt to deserialize the
/// keys and values in the order they are defined in the document.
#[derive(Debug)]
pub struct TomlIndexMap<K, V>(IndexMap<K, V>);

impl<K, V> TomlIndexMap<K, V> {
    pub fn into_inner(self) -> IndexMap<K, V> {
        self.0
    }
}

impl<K, V> From<TomlIndexMap<K, V>> for IndexMap<K, V> {
    fn from(value: TomlIndexMap<K, V>) -> Self {
        value.0
    }
}

impl<'de, K: FromKey<'de> + Hash + Eq, V: toml_span::Deserialize<'de>> toml_span::Deserialize<'de>
    for TomlIndexMap<K, V>
where
    <K as FromKey<'de>>::Err: std::fmt::Display,
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        Ok(Self(TomlIndexMap::<K, Same>::deserialize_as(value)?))
    }
}

impl<'de, K: FromKey<'de> + Hash + Eq, T, U> DeserializeAs<'de, IndexMap<K, T>>
    for TomlIndexMap<K, U>
where
    <K as FromKey<'de>>::Err: std::fmt::Display,
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<IndexMap<K, T>, DeserError> {
        match value.take() {
            ValueInner::Table(table) => {
                let mut errors = DeserError { errors: Vec::new() };
                let mut result = IndexMap::new();
                for (key, mut value) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
                    let key_span = key.span;
                    let key = K::from_key(key).map_err(|e: <K as FromKey>::Err| Error {
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
            other => Err(DeserError::from(expected("a table", other, value.span))),
        }
    }
}

#[cfg(test)]
mod test {
    use insta::assert_debug_snapshot;
    use toml_span::Deserialize;

    use super::*;

    #[test]
    pub fn test_index_map_retains_order() {
        let mut result = toml_span::parse(
            r#"
        b = 1
        c = 2
        a = 3
        d = 4
        "#,
        )
        .unwrap();
        let result = TomlIndexMap::<String, i32>::deserialize(&mut result)
            .unwrap()
            .into_inner();
        assert_debug_snapshot!(result, @r###"
        {
            "b": 1,
            "c": 2,
            "a": 3,
            "d": 4,
        }
        "###);
    }
}
