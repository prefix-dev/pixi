use std::marker::PhantomData;

use toml_span::{DeserError, Value, value::ValueInner};

use crate::DeserializeAs;

/// A deserializer helper that will deserialize either a single value or a
/// sequence of values as a `Vec<T>`.
pub struct OneOrMany<T>(PhantomData<T>);

impl<'de, T, U> DeserializeAs<'de, Vec<T>> for OneOrMany<U>
where
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<Vec<T>, DeserError> {
        match value.take() {
            ValueInner::Array(arr) => arr
                .into_iter()
                .map(|mut v| U::deserialize_as(&mut v))
                .collect(),
            inner => {
                let mut value = Value::with_span(inner, value.span);
                Ok(vec![U::deserialize_as(&mut value)?])
            }
        }
    }
}
