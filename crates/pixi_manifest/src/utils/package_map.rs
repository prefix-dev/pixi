use crate::utils::PixiSpanned;
use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use serde::{
    de::{DeserializeSeed, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::ops::Range;
use std::{fmt, marker::PhantomData};

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

impl<'de, 'a> DeserializeSeed<'de> for PackageMap<'a> {
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
