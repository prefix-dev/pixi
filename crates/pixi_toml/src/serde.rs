//! Provides a bridge to deserialize types that implement `serde::Deserialize` but not `toml_span::Deserialize`.

use std::collections::BTreeMap;

use serde::Deserialize;
use toml_span::{DeserError, ErrorKind, Value, value::ValueInner};

use crate::DeserializeAs;

/// A wrapper type that enables deserializing any type that implements `serde::Deserialize`
/// from a TOML value.
///
/// This is useful when you have types from external crates that already implement
/// `serde::Deserialize` but not `toml_span::Deserialize`.
///
/// # Example
/// ```ignore
/// use pixi_toml::{Serde, TomlWith};
///
/// // Assuming ExternalType implements serde::Deserialize but not toml_span::Deserialize
/// let value = th.optional::<TomlWith<_, Serde<ExternalType>>>("field")
///     .map(TomlWith::into_inner);
/// ```
pub struct Serde<T>(std::marker::PhantomData<T>);

impl<'de, T> DeserializeAs<'de, T> for Serde<T>
where
    T: Deserialize<'de>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<T, DeserError> {
        // Convert TOML value to serde value
        let value_as_json = convert_toml_to_serde(value)?;

        // Deserialize using T's serde::Deserialize implementation
        T::deserialize(serde_value::ValueDeserializer::new(value_as_json)).map_err(
            |e: serde_value::DeserializerError| {
                DeserError::from(toml_span::Error {
                    kind: ErrorKind::Custom(e.to_string().into()),
                    span: value.span,
                    line_info: None,
                })
            },
        )
    }
}

/// Convert a TOML value to a serde value
pub fn convert_toml_to_serde(value: &mut Value) -> Result<serde_value::Value, DeserError> {
    Ok(match value.take() {
        ValueInner::String(s) => serde_value::Value::String(s.to_string()),
        ValueInner::Integer(i) => serde_value::Value::I64(i),
        ValueInner::Float(f) => serde_value::Value::F64(f),
        ValueInner::Boolean(b) => serde_value::Value::Bool(b),
        ValueInner::Array(mut arr) => {
            let mut json_arr = Vec::new();
            for item in &mut arr {
                json_arr.push(convert_toml_to_serde(item)?);
            }
            serde_value::Value::Seq(json_arr)
        }
        ValueInner::Table(table) => {
            let mut map = BTreeMap::new();
            for (key, mut val) in table {
                map.insert(
                    serde_value::Value::String(key.to_string()),
                    convert_toml_to_serde(&mut val)?,
                );
            }
            serde_value::Value::Map(map)
        }
    })
}
