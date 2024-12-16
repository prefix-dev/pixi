use std::{hash::Hash, str::FromStr};

use indexmap::IndexMap;
use itertools::Itertools;
use toml_span::{
    value::{Table, ValueInner},
    DeserError, Error, ErrorKind, Value,
};

/// [`IndexMap`] is not supported by `toml_span` so we need to implement our own
/// deserializer.
///
/// The deserializer will expect a table and will attempt to deserialize the
/// keys and values in the order they are defined in the document.
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

impl<'de, K: FromStr + Hash + Eq, V: toml_span::Deserialize<'de>> toml_span::Deserialize<'de>
    for TomlIndexMap<K, V>
where
    <K as FromStr>::Err: std::fmt::Display,
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::Table(table) => Self::from_table(table),
            other => Err(DeserError::from(Error {
                kind: ErrorKind::Wanted {
                    expected: "table".into(),
                    found: other.type_str().into(),
                },
                span: value.span,
                line_info: None,
            })),
        }
    }
}

impl<'de, K: FromStr + Hash + Eq, V: toml_span::Deserialize<'de>> TomlIndexMap<K, V>
where
    <K as FromStr>::Err: std::fmt::Display,
{
    pub fn from_table(table: Table<'de>) -> Result<Self, DeserError> {
        let mut errors = DeserError { errors: Vec::new() };
        let mut result = IndexMap::new();
        for (key, mut value) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
            let key = key.name.parse().map_err(|e: <K as FromStr>::Err| Error {
                kind: ErrorKind::Custom(e.to_string().into()),
                span: key.span,
                line_info: None,
            });

            let value = V::deserialize(&mut value);

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
            Ok(Self(result))
        } else {
            Err(errors)
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
        let result = TomlIndexMap::<String, i32>::deserialize(&mut result);
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
