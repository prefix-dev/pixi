use std::{fmt, marker::PhantomData, ops::Range, str::FromStr};

use indexmap::IndexMap;
use itertools::Itertools;
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;
use serde::{
    de::{DeserializeSeed, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use toml_span::{de_helpers::expected, value::ValueInner, DeserError, Span, Value};

use crate::utils::PixiSpanned;

#[derive(Clone, Default, Debug, Serialize)]
pub struct UniquePackageMap {
    #[serde(flatten)]
    pub specs: IndexMap<rattler_conda_types::PackageName, PixiSpec>,

    #[serde(skip)]
    pub name_spans: IndexMap<rattler_conda_types::PackageName, Range<usize>>,

    #[serde(skip)]
    pub value_spans: IndexMap<rattler_conda_types::PackageName, Range<usize>>,
}

impl From<UniquePackageMap> for IndexMap<rattler_conda_types::PackageName, PixiSpec> {
    fn from(value: UniquePackageMap) -> Self {
        value.specs
    }
}

impl IntoIterator for UniquePackageMap {
    type Item = (rattler_conda_types::PackageName, PixiSpec);
    type IntoIter = indexmap::map::IntoIter<rattler_conda_types::PackageName, PixiSpec>;

    fn into_iter(self) -> Self::IntoIter {
        self.specs.into_iter()
    }
}

impl<'de> Deserialize<'de> for UniquePackageMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PackageMapVisitor(PhantomData<()>);

        impl<'de> Visitor<'de> for PackageMapVisitor {
            type Value = UniquePackageMap;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "a map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = UniquePackageMap::default();
                while let Some((package_name, spec)) = map.next_entry_seed::<PackageMap, _>(
                    PackageMap(&result.specs),
                    PhantomData::<PixiSpanned<PixiSpec>>,
                )? {
                    let PixiSpanned {
                        span: package_name_span,
                        value: package_name,
                    } = package_name;
                    let PixiSpanned {
                        span: spec_span,
                        value: spec,
                    } = spec;
                    if let Some(package_name_span) = package_name_span {
                        result
                            .name_spans
                            .insert(package_name.clone(), package_name_span);
                    }
                    if let Some(spec_span) = spec_span {
                        result.value_spans.insert(package_name.clone(), spec_span);
                    }
                    result.specs.insert(package_name, spec);
                }

                Ok(result)
            }
        }
        let visitor = PackageMapVisitor(PhantomData);
        deserializer.deserialize_map(visitor)
    }
}

struct PackageMap<'a>(&'a IndexMap<rattler_conda_types::PackageName, PixiSpec>);

impl<'de> DeserializeSeed<'de> for PackageMap<'_> {
    type Value = PixiSpanned<rattler_conda_types::PackageName>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let package_name = Self::Value::deserialize(deserializer)?;
        match self.0.get_key_value(&package_name.value) {
            Some((package_name, _)) => {
                Err(serde::de::Error::custom(
                    format!(
                        "duplicate dependency: {} (please avoid using capitalized names for the dependencies)", package_name.as_source())
                ))
            }
            None => Ok(package_name),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for UniquePackageMap {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let table = match value.take() {
            ValueInner::Table(table) => table,
            inner => return Err(expected("a table", inner, value.span).into()),
        };

        let mut errors = DeserError { errors: vec![] };
        let mut result = Self::default();
        for (key, mut value) in table.into_iter().sorted_by_key(|(k, _)| k.span.start) {
            let name = match PackageName::from_str(&key.name) {
                Ok(name) => {
                    if let Some(first) = result.name_spans.get(&name) {
                        errors.errors.push(toml_span::Error {
                            kind: toml_span::ErrorKind::DuplicateKey {
                                key: key.name.into_owned(),
                                first: Span {
                                    start: first.start,
                                    end: first.end,
                                },
                            },
                            span: key.span,
                            line_info: None,
                        });
                        None
                    } else {
                        Some(name)
                    }
                }
                Err(e) => {
                    errors.errors.push(toml_span::Error {
                        kind: toml_span::ErrorKind::Custom(e.to_string().into()),
                        span: key.span,
                        line_info: None,
                    });
                    None
                }
            };

            let spec: Option<PixiSpec> = match toml_span::Deserialize::deserialize(&mut value) {
                Ok(spec) => Some(spec),
                Err(e) => {
                    errors.merge(e);
                    None
                }
            };

            if let (Some(name), Some(spec)) = (name, spec) {
                result.specs.insert(name.clone(), spec);
                result
                    .name_spans
                    .insert(name.clone(), key.span.start..key.span.end);
                result
                    .value_spans
                    .insert(name, value.span.start..value.span.end);
            }
        }

        if errors.errors.is_empty() {
            Ok(result)
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;

    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

    #[test]
    pub fn test_duplicate_package_name() {
        let input = r#"
        foo = "1.0"
        bar = "2.0"
        Foo = "1.0"
        "#;
        assert_snapshot!(format_parse_error(
            input,
            UniquePackageMap::from_toml_str(input).unwrap_err()
        ));
    }
}
