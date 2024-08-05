use rattler_conda_types::NamelessMatchSpec;
use rattler_conda_types::ParseStrictness::{Lenient, Strict};
use serde::de::DeserializeSeed;
use serde::{Deserialize, Deserializer};

pub(crate) struct NamelessMatchSpecWrapper {}

impl<'de, 'a> DeserializeSeed<'de> for &'a NamelessMatchSpecWrapper {
    type Value = NamelessMatchSpec;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .string(|str| {
                match NamelessMatchSpec::from_str(str, Strict) {
                    Ok(spec) => Ok(spec),
                    Err(_) => {
                        let spec = NamelessMatchSpec::from_str(str, Lenient).map_err(serde::de::Error::custom)?;
                        tracing::warn!("Parsed '{str}' as '{spec}', in a future version this will become an error.", spec=&spec);
                        Ok(spec)
                    }
                }
            })
            .map(|map| {
                NamelessMatchSpec::deserialize(serde::de::value::MapAccessDeserializer::new(map))
            })
            .expecting("either a map or a string")
            .deserialize(deserializer)
    }
}
