use std::{
    collections::HashMap,
    hash::{BuildHasher, Hash, RandomState},
    str::FromStr,
};

use toml_span::{value::ValueInner, DeserError, ErrorKind, Value};

/// [`IndexMap`] is not supported by `toml_span` so we need to implement our own
/// deserializer.
///
/// The deserializer will expect a table and will attempt to deserialize the
/// keys and values in the order they are defined in the document.
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

impl<'de, K: FromStr + Hash + Eq, V: toml_span::Deserialize<'de>, H> toml_span::Deserialize<'de>
    for TomlHashMap<K, V, H>
where
    <K as FromStr>::Err: std::fmt::Display,
    H: Default + BuildHasher,
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let table = match value.take() {
            ValueInner::Table(table) => table,
            other => {
                return Err(DeserError::from(toml_span::Error {
                    kind: ErrorKind::Wanted {
                        expected: "table".into(),
                        found: other.type_str().into(),
                    },
                    span: value.span,
                    line_info: None,
                }))
            }
        };

        let mut errors = DeserError { errors: Vec::new() };
        let mut result = HashMap::default();
        for (key, mut value) in table.into_iter() {
            let key = key
                .name
                .parse()
                .map_err(|e: <K as FromStr>::Err| toml_span::Error {
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
