mod from_str;
mod index_map;
mod variant;
mod with;

pub use from_str::TomlFromStr;
pub use index_map::TomlIndexMap;
use toml_span::{value::ValueInner, DeserError, Error, ErrorKind, Value};
pub use variant::TomlEnum;
pub use with::TomlWith;

/// A trait that enables efficient deserialization of one type into another.
pub trait DeserializeAs<'de, T> {
    fn deserialize_as(value: &mut Value<'de>) -> Result<T, DeserError>;
}

impl<'de, T, U> DeserializeAs<'de, Vec<T>> for Vec<U>
where
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<Vec<T>, DeserError> {
        let array = match value.take() {
            ValueInner::Array(array) => array,
            other => {
                return Err(DeserError::from(Error {
                    kind: ErrorKind::Wanted {
                        expected: "array".into(),
                        found: other.type_str().into(),
                    },
                    span: value.span,
                    line_info: None,
                }))
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
