use indexmap::IndexSet;
use rip::{Extra, VersionSpecifiers};
use serde::{Deserialize, Deserializer};

/// Describes a dependency on a python package. This does not actually include the package name
/// itself as this is used in a hashmap.
#[derive(Debug, Clone)]
pub struct PythonRequirement {
    pub version: VersionSpecifiers,
    pub extras: IndexSet<Extra>,
}

impl From<VersionSpecifiers> for PythonRequirement {
    fn from(value: VersionSpecifiers) -> Self {
        Self {
            version: value,
            extras: Default::default(),
        }
    }
}

impl<'de> Deserialize<'de> for PythonRequirement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawRequirement {
            OnlyVersion(VersionSpecifiers),
            ExtendedForm {
                version: VersionSpecifiers,
                #[serde(default)]
                extras: IndexSet<Extra>,
            },
        }

        match RawRequirement::deserialize(deserializer)? {
            RawRequirement::OnlyVersion(version) => Ok(version.into()),
            RawRequirement::ExtendedForm { version, extras } => Ok(Self { version, extras }),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use insta::assert_debug_snapshot;
    use rip::PackageName;
    use std::collections::HashMap;

    #[test]
    fn test_only_version() {
        let requirement: HashMap<PackageName, PythonRequirement> =
            toml_edit::de::from_str("foo = \">=3.12\"").unwrap();
        assert_debug_snapshot!(requirement);
    }

    #[test]
    fn test_extended() {
        let requirement: HashMap<PackageName, PythonRequirement> =
            toml::de::from_str("foo = { version=\">=3.12\", extras = [\"bar\"] }").unwrap();
        assert_debug_snapshot!(requirement);
    }
}
