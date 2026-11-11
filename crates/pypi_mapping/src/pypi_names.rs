//! [`PypiNames`] — the value of one conda-to-pypi mapping entry.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// The PyPI equivalents of one conda package.
///
/// Mapping documents spell this as a single name (`"numpy"`), a list of
/// names (`["airflow", "apache-airflow"]`), or `null` ("known not to be on
/// PyPI"). All forms normalize to a list; an empty list means the package
/// has no PyPI equivalent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PypiNames(pub Vec<String>);

impl<'de> Deserialize<'de> for PypiNames {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PypiNamesVisitor;

        impl<'de> de::Visitor<'de> for PypiNamesVisitor {
            type Value = PypiNames;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a pypi name, a list of pypi names, or null")
            }

            fn visit_str<E: de::Error>(self, name: &str) -> Result<Self::Value, E> {
                Ok(PypiNames(vec![name.to_owned()]))
            }

            fn visit_string<E: de::Error>(self, name: String) -> Result<Self::Value, E> {
                Ok(PypiNames(vec![name]))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(PypiNames(Vec::new()))
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(PypiNames(Vec::new()))
            }

            fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
            where
                D2: Deserializer<'de>,
            {
                deserializer.deserialize_any(PypiNamesVisitor)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut names = Vec::with_capacity(seq.size_hint().unwrap_or(1));
                while let Some(name) = seq.next_element::<String>()? {
                    names.push(name);
                }
                Ok(PypiNames(names))
            }
        }

        deserializer.deserialize_any(PypiNamesVisitor)
    }
}

impl Serialize for PypiNames {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn parse(json: &str) -> HashMap<String, PypiNames> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn test_deserializes_single_name() {
        let mapping = parse(r#"{"numpy": "my-numpy"}"#);
        assert_eq!(mapping["numpy"], PypiNames(vec!["my-numpy".to_string()]));
    }

    #[test]
    fn test_deserializes_name_list() {
        let mapping = parse(r#"{"airflow": ["airflow", "apache-airflow"]}"#);
        assert_eq!(
            mapping["airflow"],
            PypiNames(vec!["airflow".to_string(), "apache-airflow".to_string()])
        );
    }

    #[test]
    fn test_deserializes_null_and_empty_list_as_not_on_pypi() {
        let mapping = parse(r#"{"a": null, "b": []}"#);
        assert_eq!(mapping["a"], PypiNames(Vec::new()));
        assert_eq!(mapping["b"], PypiNames(Vec::new()));
    }

    #[test]
    fn test_deserializes_mixed_document() {
        // Single-name, list and null entries may be mixed in one document.
        let mapping = parse(r#"{"a": "b", "c": ["d", "e"], "f": null}"#);
        assert_eq!(mapping["a"], PypiNames(vec!["b".to_string()]));
        assert_eq!(
            mapping["c"],
            PypiNames(vec!["d".to_string(), "e".to_string()])
        );
        assert_eq!(mapping["f"], PypiNames(Vec::new()));
    }

    #[test]
    fn test_rejects_non_string_values() {
        let err = serde_json::from_str::<HashMap<String, PypiNames>>(r#"{"a": 1}"#).unwrap_err();
        assert!(err.to_string().contains("a pypi name"), "{err}");
        assert!(serde_json::from_str::<HashMap<String, PypiNames>>(r#"{"a": ["b", 2]}"#).is_err());
    }

    #[test]
    fn test_serializes_as_list() {
        assert_eq!(
            serde_json::to_string(&PypiNames(vec!["b".to_string()])).unwrap(),
            r#"["b"]"#
        );
        assert_eq!(serde_json::to_string(&PypiNames(Vec::new())).unwrap(), "[]");
    }
}
