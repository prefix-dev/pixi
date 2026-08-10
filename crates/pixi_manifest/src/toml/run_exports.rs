use toml_span::{DeserError, Value, de_helpers::TableHelper};

use crate::utils::{PixiSpanned, inheritable_package_map::ConditionalInheritablePackageMap};

/// The TOML representation of the `[package.run-exports]` section.
///
/// Each bucket is a dependency table that supports the same forms as the other
/// package dependency tables: plain `name = spec` entries, `if(<expression>)`
/// conditional sub-tables, and `{ workspace = true }` inheritance.
#[derive(Debug, Default)]
pub struct TomlRunExports {
    pub noarch: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
    pub strong: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
    pub weak: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
    pub strong_constraints: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
    pub weak_constraints: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
}

impl<'de> toml_span::Deserialize<'de> for TomlRunExports {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let noarch = th.optional("noarch");
        let strong = th.optional("strong");
        let weak = th.optional("weak");
        let strong_constraints = th.optional("strong-constraints");
        let weak_constraints = th.optional("weak-constraints");
        th.finalize(None)?;
        Ok(TomlRunExports {
            noarch,
            strong,
            weak,
            strong_constraints,
            weak_constraints,
        })
    }
}
