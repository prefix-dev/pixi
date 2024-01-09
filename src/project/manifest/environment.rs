use crate::consts;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

/// The name of an environment. This is either a string or default for the default environment.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum EnvironmentName {
    Default,
    Named(String),
}

impl EnvironmentName {
    /// Returns the name of the environment. This is either the name of the environment or the name
    /// of the default environment.
    pub fn as_str(&self) -> &str {
        match self {
            EnvironmentName::Default => consts::DEFAULT_ENVIRONMENT_NAME,
            EnvironmentName::Named(name) => name.as_str(),
        }
    }
}

impl<'de> Deserialize<'de> for EnvironmentName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        Ok(EnvironmentName::Named(name))
    }
}

/// An environment describes a set of features that are available together.
///
/// Individual features cannot be used directly, instead they are grouped together into
/// environments. Environments are then locked and installed.
#[derive(Debug, Clone)]
pub struct Environment {
    /// The name of the environment
    pub name: EnvironmentName,

    /// The names of the features that together make up this environment.
    ///
    /// Note that the default feature is always added to the set of features that make up the
    /// environment.
    pub features: Vec<String>,

    /// The optional location of where the features are defined in the manifest toml.
    pub features_source_loc: Option<std::ops::Range<usize>>,

    /// An optional solver-group. Multiple environments can share the same solve-group. All the
    /// dependencies of the environment that share the same solve-group will be solved together.
    pub solve_group: Option<String>,
}

impl<'de> Deserialize<'de> for Environment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(EnvironmentVisitor)
    }
}

struct EnvironmentVisitor;

impl<'de> Visitor<'de> for EnvironmentVisitor {
    type Value = Environment;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a list of features or a map with additional fields")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        let mut features = Vec::new();
        while let Some(feature) = seq.next_element()? {
            features.push(feature);
        }
        Ok(Environment {
            name: EnvironmentName::Default, // Adjusted by manifest deserialization
            features,
            features_source_loc: None,
            solve_group: None,
        })
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut features = None;
        let mut solve_group = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "features" => {
                    if features.is_some() {
                        return Err(de::Error::duplicate_field("features"));
                    }
                    features = Some(map.next_value()?); // Deserialize the value associated with the key
                }
                "solve-group" => {
                    if solve_group.is_some() {
                        return Err(de::Error::duplicate_field("solve-group"));
                    }
                    let sg: String = map.next_value()?; // Directly deserialize the value as a String
                    solve_group = Some(sg);
                }
                _ => return Err(de::Error::unknown_field(&key, &["features", "solve-group"])),
            }
        }
        let features = features.ok_or_else(|| de::Error::missing_field("features"))?;
        Ok(Environment {
            name: EnvironmentName::Default, // Adjusted by manifest deserialization
            features,
            features_source_loc: None,
            solve_group,
        })
    }
}
