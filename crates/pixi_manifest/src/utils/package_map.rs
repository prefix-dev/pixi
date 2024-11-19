use std::{fmt, marker::PhantomData};

use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;
use serde::{
    de::{DeserializeSeed, MapAccess, Visitor},
    Deserialize, Deserializer,
};

struct PackageMap<'a>(&'a IndexMap<PackageName, PixiSpec>);

impl<'de, 'a> DeserializeSeed<'de> for PackageMap<'a> {
    type Value = PackageName;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let package_name = PackageName::deserialize(deserializer)?;
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

pub fn deserialize_package_map<'de, D>(
    deserializer: D,
) -> Result<IndexMap<PackageName, PixiSpec>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PackageMapVisitor(PhantomData<()>);

    impl<'de> Visitor<'de> for PackageMapVisitor {
        type Value = IndexMap<PackageName, PixiSpec>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a map")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut result = IndexMap::new();
            while let Some((package_name, spec)) =
                map.next_entry_seed::<PackageMap, _>(PackageMap(&result), PhantomData::<PixiSpec>)?
            {
                result.insert(package_name, spec);
            }

            Ok(result)
        }
    }
    let visitor = PackageMapVisitor(PhantomData);
    deserializer.deserialize_seq(visitor)
}

pub fn deserialize_opt_package_map<'de, D>(
    deserializer: D,
) -> Result<Option<IndexMap<PackageName, PixiSpec>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(deserialize_package_map(deserializer)?))
}
