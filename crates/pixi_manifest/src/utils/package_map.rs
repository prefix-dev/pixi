use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use serde::{
    de::{DeserializeSeed, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::ops::DerefMut;
use std::{fmt, marker::PhantomData, ops::Deref};

#[derive(Clone, Default, Debug, Serialize)]
#[serde(transparent)]
pub struct UniquePackageMap(IndexMap<rattler_conda_types::PackageName, PixiSpec>);

impl From<UniquePackageMap> for IndexMap<rattler_conda_types::PackageName, PixiSpec> {
    fn from(value: UniquePackageMap) -> Self {
        value.0
    }
}

impl Deref for UniquePackageMap {
    type Target = IndexMap<rattler_conda_types::PackageName, PixiSpec>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for UniquePackageMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'de> Deserialize<'de> for UniquePackageMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PackageMapVisitor(PhantomData<()>);

        impl<'de> Visitor<'de> for PackageMapVisitor {
            type Value = IndexMap<rattler_conda_types::PackageName, PixiSpec>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "a map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = IndexMap::new();
                while let Some((package_name, spec)) = map.next_entry_seed::<PackageMap, _>(
                    PackageMap(&result),
                    PhantomData::<PixiSpec>,
                )? {
                    result.insert(package_name, spec);
                }

                Ok(result)
            }
        }
        let visitor = PackageMapVisitor(PhantomData);
        let packages = deserializer.deserialize_map(visitor)?;
        Ok(UniquePackageMap(packages))
    }
}

struct PackageMap<'a>(&'a IndexMap<rattler_conda_types::PackageName, PixiSpec>);

impl<'de, 'a> DeserializeSeed<'de> for PackageMap<'a> {
    type Value = rattler_conda_types::PackageName;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let package_name = rattler_conda_types::PackageName::deserialize(deserializer)?;
        match self.0.get_key_value(&package_name) {
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
