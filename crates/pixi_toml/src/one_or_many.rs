use std::marker::PhantomData;

use toml_span::{value::ValueInner, DeserError, Value};

use crate::DeserializeAs;

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
