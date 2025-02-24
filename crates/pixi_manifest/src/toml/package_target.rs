use toml_span::{de_helpers::TableHelper, DeserError, Value};

use crate::{
    target::PackageTarget,
    toml::target::combine_target_dependencies,
    utils::{package_map::UniquePackageMap, PixiSpanned},
    KnownPreviewFeature, Preview, SpecType, TomlError,
};

#[derive(Debug)]
pub struct TomlPackageTarget {
    pub run_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
}

impl<'de> toml_span::Deserialize<'de> for TomlPackageTarget {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let run_dependencies = th.optional("run-dependencies");
        let host_dependencies = th.optional("host-dependencies");
        let build_dependencies = th.optional("build-dependencies");
        th.finalize(None)?;
        Ok(TomlPackageTarget {
            run_dependencies,
            host_dependencies,
            build_dependencies,
        })
    }
}

impl TomlPackageTarget {
    pub fn into_package_target(self, preview: &Preview) -> Result<PackageTarget, TomlError> {
        Ok(PackageTarget {
            dependencies: combine_target_dependencies(
                [
                    (SpecType::Run, self.run_dependencies),
                    (SpecType::Host, self.host_dependencies),
                    (SpecType::Build, self.build_dependencies),
                ],
                preview.is_enabled(KnownPreviewFeature::PixiBuild),
            )?,
        })
    }
}
