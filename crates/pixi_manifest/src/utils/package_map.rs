use std::{fmt, marker::PhantomData, ops::Range, str::FromStr};

use indexmap::IndexMap;
use itertools::Itertools;
use pixi_spec::{BinarySpec, PixiSpec, SourceSpec};
use rattler_conda_types::PackageName;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeSeed, MapAccess, Visitor},
};
use toml_span::{
    DeserError, Span, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::{TomlError, error::GenericError, toml::TomlPackage, utils::PixiSpanned};

#[derive(Clone, Default, Debug, Serialize)]
pub struct UniquePackageMap {
    #[serde(flatten)]
    pub specs: IndexMap<rattler_conda_types::PackageName, PixiSpec>,

    #[serde(skip)]
    pub name_spans: IndexMap<rattler_conda_types::PackageName, Range<usize>>,

    #[serde(skip)]
    pub value_spans: IndexMap<rattler_conda_types::PackageName, Range<usize>>,
}

impl UniquePackageMap {
    pub fn into_inner(
        self,
        is_pixi_build_enabled: bool,
    ) -> Result<IndexMap<rattler_conda_types::PackageName, PixiSpec>, TomlError> {
        if !is_pixi_build_enabled
            && let Some((package_name, _)) = self.specs.iter().find(|(_, spec)| spec.is_source())
        {
            return Err(TomlError::Generic(
                    GenericError::new(
                        "conda source dependencies are not allowed without enabling the 'pixi-build' preview feature",
                    )
                    .with_opt_span(self.value_spans.get(package_name).cloned())
                    .with_span_label("source dependency specified here")
                    .with_help(
                        "Add `preview = [\"pixi-build\"]` to the `workspace` or `project` table of your manifest",
                    ),
                ));
        }
        Ok(self.specs)
    }
}

impl IntoIterator for UniquePackageMap {
    type Item = (rattler_conda_types::PackageName, PixiSpec);
    type IntoIter = indexmap::map::IntoIter<rattler_conda_types::PackageName, PixiSpec>;

    fn into_iter(self) -> Self::IntoIter {
        self.specs.into_iter()
    }
}

impl Extend<(rattler_conda_types::PackageName, PixiSpec)> for UniquePackageMap {
    fn extend<T: IntoIterator<Item = (rattler_conda_types::PackageName, PixiSpec)>>(
        &mut self,
        iter: T,
    ) {
        for (name, spec) in iter {
            self.specs.insert(name, spec);
            // Note: We don't set spans here as they're primarily used for TOML parsing
        }
    }
}

impl Extend<(rattler_conda_types::PackageName, SourceSpec)> for UniquePackageMap {
    fn extend<T: IntoIterator<Item = (rattler_conda_types::PackageName, SourceSpec)>>(
        &mut self,
        iter: T,
    ) {
        for (name, spec) in iter {
            self.specs.insert(name, spec.into());
            // Note: We don't set spans here as they're primarily used for TOML parsing
        }
    }
}

impl Extend<(rattler_conda_types::PackageName, BinarySpec)> for UniquePackageMap {
    fn extend<T: IntoIterator<Item = (rattler_conda_types::PackageName, BinarySpec)>>(
        &mut self,
        iter: T,
    ) {
        for (name, spec) in iter {
            self.specs.insert(name, spec.into());
            // Note: We don't set spans here as they're primarily used for TOML parsing
        }
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
            Some((package_name, _)) => Err(serde::de::Error::custom(format!(
                "duplicate dependency: {} (please avoid using capitalized names for the dependencies)",
                package_name.as_source()
            ))),
            None => Ok(package_name),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for UniquePackageMap {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        Ok(deserialize_dependency_table(value, InlinePackages::Deny)?.specs)
    }
}

/// Whether a dependency table accepts inline package definitions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InlinePackages {
    /// Peel `package` sub-tables off each spec and collect them.
    Allow,
    /// Leave any `package` key in place so the spec parser rejects it.
    Deny,
}

/// A dependency table that may carry inline package definitions.
///
/// Behaves like a [`UniquePackageMap`] but additionally captures any inline
/// package definitions (`package = { ... }`) attached to individual dependency
/// specs. An inline definition describes a conda source dependency's package,
/// so it is only accepted next to a `git`, `path` or `url` source. The conda
/// dependency tables (`[dependencies]`, `[host-dependencies]`,
/// `[build-dependencies]`) use this type because that is where source
/// dependencies may appear.
#[derive(Default, Debug)]
pub struct DependencyTable {
    /// The dependency specs with any inline `package` keys peeled off.
    pub specs: UniquePackageMap,

    /// Inline `package` tables keyed by dependency name. Each is the parsed
    /// `package = { ... }` sub-table of the matching source spec.
    pub inline_packages: IndexMap<rattler_conda_types::PackageName, PixiSpanned<TomlPackage>>,
}

impl<'de> toml_span::Deserialize<'de> for DependencyTable {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        deserialize_dependency_table(value, InlinePackages::Allow)
    }
}

/// Deserializes a table of `name = <spec>` pairs into a [`DependencyTable`],
/// optionally peeling inline package definitions off each value.
///
/// When `inline` is [`InlinePackages::Allow`], a `package` sub-table is removed
/// from each value before the remaining keys are parsed as a [`PixiSpec`], and
/// the parsed [`TomlPackage`] is collected into
/// [`DependencyTable::inline_packages`] keyed by the same dependency name. When
/// it is [`InlinePackages::Deny`] the value is parsed verbatim, leaving any
/// `package` key to be rejected by the spec parser, and the returned
/// `inline_packages` is empty.
fn deserialize_dependency_table<'de>(
    value: &mut Value<'de>,
    inline: InlinePackages,
) -> Result<DependencyTable, DeserError> {
    let table = match value.take() {
        ValueInner::Table(table) => table,
        inner => return Err(expected("a table", inner, value.span).into()),
    };

    let mut errors = DeserError { errors: vec![] };
    let mut result = UniquePackageMap::default();
    let mut inline_packages = IndexMap::new();
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

        // Peel off an inline package definition when the surrounding table
        // permits it. This happens before spec parsing because `TomlSpec`
        // rejects unknown keys.
        let inline_package = match inline {
            InlinePackages::Allow => match peel_inline_package(&mut value) {
                Ok(inline) => inline,
                Err(e) => {
                    errors.merge(e);
                    None
                }
            },
            InlinePackages::Deny => None,
        };

        let spec: Option<PixiSpec> = match toml_span::Deserialize::deserialize(&mut value) {
            Ok(spec) => Some(spec),
            Err(e) => {
                errors.merge(e);
                None
            }
        };

        // An inline package definition describes how to build source code, so
        // the surrounding spec must point at a git, path or url source.
        if inline_package.is_some()
            && let Some(spec) = spec.as_ref()
            && !spec.is_source()
        {
            errors.errors.push(toml_span::Error {
                kind: toml_span::ErrorKind::Custom(
                    "an inline package definition requires a `git`, `path` or `url` source location"
                        .into(),
                ),
                span: value.span,
                line_info: None,
            });
        }

        if let (Some(name), Some(spec)) = (name, spec) {
            result.specs.insert(name.clone(), spec);
            result
                .name_spans
                .insert(name.clone(), key.span.start..key.span.end);
            result
                .value_spans
                .insert(name.clone(), value.span.start..value.span.end);
            if let Some(package) = inline_package {
                inline_packages.insert(name, package);
            }
        }
    }

    if errors.errors.is_empty() {
        Ok(DependencyTable {
            specs: result,
            inline_packages,
        })
    } else {
        Err(errors)
    }
}

/// Peels an inline package definition off a single dependency value.
///
/// Returns the parsed [`TomlPackage`] when `value` is a table containing a
/// `package` key, leaving `value` holding the remaining keys (the source spec).
/// Returns `None` otherwise. Rejects an explicit `package.name` (the name comes
/// from the dependency key) and a `package.build.source` (the source comes from
/// the surrounding spec).
pub(crate) fn peel_inline_package<'de>(
    value: &mut Value<'de>,
) -> Result<Option<PixiSpanned<TomlPackage>>, DeserError> {
    if value
        .as_table()
        .is_none_or(|table| !table.contains_key("package"))
    {
        return Ok(None);
    }

    let mut th = TableHelper::new(value)?;
    let package = th.take("package");
    th.finalize(Some(value))?;

    let Some((_, mut package_value)) = package else {
        return Ok(None);
    };
    let span = package_value.span;

    let package = <TomlPackage as toml_span::Deserialize>::deserialize(&mut package_value)?;

    if package.name.is_some() {
        return Err(toml_span::Error {
            kind: toml_span::ErrorKind::Custom(
                "an inline package definition cannot set `name`; it is taken from the dependency key"
                    .into(),
            ),
            span,
            line_info: None,
        }
        .into());
    }

    if package.build.source.is_some() {
        return Err(toml_span::Error {
            kind: toml_span::ErrorKind::Custom(
                "an inline package definition cannot set `build.source`; the source is taken from the dependency spec"
                    .into(),
            ),
            span,
            line_info: None,
        }
        .into());
    }

    Ok(Some(PixiSpanned {
        span: Some(span.start..span.end),
        value: package,
    }))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::toml::FromTomlStr;
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;

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

    /// An inline package definition is peeled off and the remaining keys parse
    /// as the source spec.
    #[test]
    fn test_inline_package_basic() {
        let input = r#"
        rust-package = { git = "https://github.com/user/repo.git", package.build = { backend = { name = "pixi-build-rust", version = "1.0" } } }
        "#;
        let table = DependencyTable::from_toml_str(input).unwrap();

        let name = PackageName::from_str("rust-package").unwrap();
        let spec = table.specs.specs.get(&name).expect("spec retained");
        assert!(spec.is_source(), "the spec should remain a source spec");

        let inline = table
            .inline_packages
            .get(&name)
            .expect("inline package captured");
        assert_eq!(
            inline.value.build.backend.value.name.value.as_normalized(),
            "pixi-build-rust"
        );
    }

    /// The dotted `package.build` form is sugar for the full `[package]` table,
    /// whose own dependency tables are parsed too.
    #[test]
    fn test_inline_package_full_table() {
        let input = r#"
        rust-package = { git = "https://github.com/user/repo.git", package = { build = { backend = { name = "pixi-build-rust", version = "1.0" } }, run-dependencies = { foo = "*" } } }
        "#;
        let table = DependencyTable::from_toml_str(input).unwrap();
        let name = PackageName::from_str("rust-package").unwrap();
        let inline = table.inline_packages.get(&name).expect("inline captured");
        assert!(
            inline.value.run_dependencies.is_some(),
            "package run-dependencies should be parsed"
        );
    }

    /// `package.name` is taken from the dependency key and may not be set.
    #[test]
    fn test_inline_package_rejects_explicit_name() {
        let input = r#"
        rust-package = { git = "https://x/y.git", package = { name = "other", build = { backend = { name = "b", version = "1.0" } } } }
        "#;
        let err = DependencyTable::from_toml_str(input).unwrap_err();
        assert!(
            format_parse_error(input, err).contains("cannot set `name`"),
            "expected a name-rejection error"
        );
    }

    /// The source comes from the surrounding spec, so `package.build.source` is
    /// rejected.
    #[test]
    fn test_inline_package_rejects_build_source() {
        let input = r#"
        rust-package = { git = "https://x/y.git", package = { build = { backend = { name = "b", version = "1.0" }, source = { path = "elsewhere" } } } }
        "#;
        let err = DependencyTable::from_toml_str(input).unwrap_err();
        assert!(
            format_parse_error(input, err).contains("cannot set `build.source`"),
            "expected a build.source-rejection error"
        );
    }

    /// An inline definition without a source location is meaningless.
    #[test]
    fn test_inline_package_requires_source_location() {
        let input = r#"
        rust-package = { version = "1.0", package.build = { backend = { name = "b", version = "1.0" } } }
        "#;
        let err = DependencyTable::from_toml_str(input).unwrap_err();
        assert!(
            format_parse_error(input, err).contains("requires a `git`, `path` or `url` source"),
            "expected a missing-source-location error"
        );
    }

    /// Tables that do not accept inline definitions ([`UniquePackageMap`]) leave
    /// the `package` key in place so it surfaces as an unexpected key.
    #[test]
    fn test_inline_package_rejected_in_unique_map() {
        let input = r#"
        rust-package = { git = "https://x/y.git", package.build = { backend = { name = "b", version = "1.0" } } }
        "#;
        assert!(
            UniquePackageMap::from_toml_str(input).is_err(),
            "inline definitions must not be accepted by UniquePackageMap"
        );
    }

    /// Inline definitions nest: an inline definition is just a package
    /// manifest without a file, so its own dependency tables accept further
    /// inline definitions. The nested definition is captured on the inner
    /// package's dependency table.
    #[test]
    fn test_inline_package_nests() {
        let input = r#"
        outer = { git = "https://x/y.git", package = { build = { backend = { name = "b", version = "1.0" } }, run-dependencies = { inner = { git = "https://x/z.git", package.build = { backend = { name = "b", version = "1.0" } } } } } }
        "#;
        let table =
            DependencyTable::from_toml_str(input).expect("a nested inline definition must parse");
        let outer = table
            .inline_packages
            .get(&PackageName::from_str("outer").unwrap())
            .expect("outer inline captured");
        let run_dependencies = outer
            .value
            .run_dependencies
            .as_ref()
            .expect("outer inline has run-dependencies");
        assert!(
            run_dependencies
                .value
                .unconditional
                .inline_packages
                .contains_key(&PackageName::from_str("inner").unwrap()),
            "the nested inline definition must be captured on the inner table"
        );
    }
}
