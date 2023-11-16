use crate::utils::spanned::PixiSpanned;
use indexmap::IndexMap;
use serde::de::{DeserializeSeed, Error, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fmt::Formatter;

/// Represents a set of python dependencies on which a project can depend. The dependencies are
/// formatted using a modern version specifier. See [`pep508_rs::modern::RequirementModern`].
#[derive(Default, Debug, Clone)]
pub struct PypiDependencies {
    requirements: IndexMap<
        PixiSpanned<rip::PackageName>,
        PixiSpanned<(pep508_rs::modern::RequirementModern, pep508_rs::Requirement)>,
    >,
}

impl PypiDependencies {
    /// Returns `true` if no requirements have been specified
    pub fn is_empty(&self) -> bool {
        self.requirements.is_empty()
    }

    /// Returns the requirements as [`pep508_rs::Requirement`]s.
    pub fn as_pep508(&self) -> Vec<pep508_rs::Requirement> {
        self.requirements
            .values()
            .map(|s| s.value.1.clone())
            .collect()
    }
}

impl<'de> Deserialize<'de> for PypiDependencies {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RequirementsVisitor;
        struct RequirementVisitor<'i> {
            name: &'i str,
        }

        impl<'de, 'i> DeserializeSeed<'de> for RequirementVisitor<'i> {
            type Value =
                PixiSpanned<(pep508_rs::modern::RequirementModern, pep508_rs::Requirement)>;

            fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                let PixiSpanned { value, span } =
                    PixiSpanned::<pep508_rs::modern::RequirementModern>::deserialize(deserializer)?;
                let req = value
                    .to_pep508(self.name, &HashMap::default())
                    .map_err(D::Error::custom)?;
                Ok(PixiSpanned {
                    value: (value, req),
                    span,
                })
            }
        }

        impl<'de> Visitor<'de> for RequirementsVisitor {
            type Value = IndexMap<
                PixiSpanned<rip::PackageName>,
                PixiSpanned<(pep508_rs::modern::RequirementModern, pep508_rs::Requirement)>,
            >;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("a mapping from package names to requirements")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = IndexMap::new();
                while let Some(key) = map.next_key::<PixiSpanned<rip::PackageName>>()? {
                    let value = map.next_value_seed(RequirementVisitor {
                        name: key.value.as_str(),
                    })?;
                    result.insert(key, value);
                }
                Ok(result)
            }
        }

        deserializer
            .deserialize_map(RequirementsVisitor {})
            .map(|requirements| PypiDependencies { requirements })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use insta::assert_debug_snapshot;

    #[test]
    fn test_only_version() {
        let requirement: PypiDependencies = toml_edit::de::from_str("foo = \">=3.12\"").unwrap();
        assert_debug_snapshot!(requirement);
    }

    #[test]
    fn test_extended() {
        let requirement: PypiDependencies =
            toml::de::from_str("foo = { version=\">=3.12\", extras = [\"bar\"] }").unwrap();
        assert_debug_snapshot!(requirement);
    }

    #[test]
    fn test_invalid_git_dependency_error() {
        let requirement = toml::de::from_str::<PypiDependencies>(
            "foo = { git=\"https://github.com/foo/bar\", branch = \"main\", rev=\"deadbeef\" }",
        );
        assert_debug_snapshot!(requirement.unwrap_err());
    }

    // TODO: This crashes currently but should instead result in an error. This should be fixed in pep508_rs.
    //#[test]
    // fn test_invalid_extra() {
    //     let requirement = toml::de::from_str::<PythonDependencies>(
    //         r#"foo = { version="3.2.1", extras=["b$r"] }"#,
    //     );
    //     assert_debug_snapshot!(requirement.unwrap_err());
    // }

    // TODO: pep508_rs does not handle ^ operator yet.
}
