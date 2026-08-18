//! [`PackageDependencySpec`]: the dependency-value type for package-level
//! dependency tables (`[package.run-dependencies]`,
//! `[package.host-dependencies]`, `[package.build-dependencies]`,
//! `[package.run-constraints]`, `[package.extra-dependencies]`, and the
//! `[package.run-exports.*]` buckets).
//!
//! This type, and *not* [`PixiSpec`] itself, is what carries the
//! `pin-subpackage` and `pin-compatible` spec values. Which tables accept
//! which pin kind is decided when a parsed table is resolved (see
//! `ResolvedPackageMap`), so every rejection carries the span of the
//! offending entry.

use pixi_spec::{BinarySpec, Pin, PixiSpec};
use pixi_toml::custom_error;
use toml_span::{DeserError, Value, value::ValueInner};

/// A single dependency value in a package-level dependency table: either a
/// regular [`PixiSpec`] (version, path, git, url, ...), a self-referential
/// `pin-subpackage` pin, or a `pin-compatible` pin against a dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageDependencySpec {
    /// A regular dependency spec, as used everywhere else in the manifest.
    Spec(PixiSpec),
    /// `{ pin-subpackage = true }` or `{ pin-subpackage = { ... } }`: a
    /// self-referential pin, resolved against the package's own
    /// `(name, version, build_string)` once it is known. Only valid when the
    /// entry's key equals the package's own name; that check happens in
    /// `TomlPackage::into_manifest`, once the package's resolved name is
    /// known (it may come from workspace inheritance).
    PinSubpackage(Pin),
    /// `{ pin-compatible = true }` or `{ pin-compatible = { ... } }`: a pin
    /// against the version of a *dependency* as resolved in the previous
    /// (build or host) environment, mirroring rattler-build's
    /// `pin_compatible()`. Only valid when the entry's key is *not* the
    /// package's own name; use `pin-subpackage` for self-pins.
    PinCompatible(Pin),
}

impl PackageDependencySpec {
    /// Returns the inner [`PixiSpec`] if this is a regular spec.
    pub fn as_spec(&self) -> Option<&PixiSpec> {
        match self {
            PackageDependencySpec::Spec(spec) => Some(spec),
            PackageDependencySpec::PinSubpackage(_) | PackageDependencySpec::PinCompatible(_) => {
                None
            }
        }
    }

    /// Returns `true` if this is a source dependency (path, git, or url).
    /// Pin entries are never source dependencies.
    pub fn is_source(&self) -> bool {
        match self {
            PackageDependencySpec::Spec(spec) => spec.is_source(),
            PackageDependencySpec::PinSubpackage(_) | PackageDependencySpec::PinCompatible(_) => {
                false
            }
        }
    }
}

impl From<PixiSpec> for PackageDependencySpec {
    fn from(value: PixiSpec) -> Self {
        PackageDependencySpec::Spec(value)
    }
}

/// Regular specs hash exactly as the bare [`PixiSpec`] did before this type
/// existed, so pin-free manifests keep their content hashes (which feed the
/// lock file through inline-package identifiers) across the pixi upgrade
/// that introduced pins. The pin variants fold in a discriminant.
impl std::hash::Hash for PackageDependencySpec {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            PackageDependencySpec::Spec(spec) => spec.hash(state),
            PackageDependencySpec::PinSubpackage(pin) => {
                1u8.hash(state);
                pin.hash(state);
            }
            PackageDependencySpec::PinCompatible(pin) => {
                2u8.hash(state);
                pin.hash(state);
            }
        }
    }
}

/// A single constraint value in a `[package.run-exports.*-constraints]`
/// bucket: a binary spec (constraints never pull in source packages) or one
/// of the two pin kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageConstraintSpec {
    /// A regular binary constraint spec.
    Binary(BinarySpec),
    /// A self-referential pin, see [`PackageDependencySpec::PinSubpackage`].
    PinSubpackage(Pin),
    /// A pin against the previous environment, see
    /// [`PackageDependencySpec::PinCompatible`].
    PinCompatible(Pin),
}

/// Same hash-stability contract as [`PackageDependencySpec`]: binary specs
/// hash as the bare [`BinarySpec`] did before.
impl std::hash::Hash for PackageConstraintSpec {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            PackageConstraintSpec::Binary(spec) => spec.hash(state),
            PackageConstraintSpec::PinSubpackage(pin) => {
                1u8.hash(state);
                pin.hash(state);
            }
            PackageConstraintSpec::PinCompatible(pin) => {
                2u8.hash(state);
                pin.hash(state);
            }
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for PackageDependencySpec {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let outer_span = value.span;
        let inner = value.take();

        // Only tables can carry a `pin-subpackage`/`pin-compatible` key;
        // strings (plain version specs) go straight to `PixiSpec`.
        let mut table = match inner {
            ValueInner::Table(table) => table,
            other => {
                // Put the value back and let `PixiSpec` handle it, including
                // producing the right "a string or a table" error for anything
                // else.
                let mut restored = Value::with_span(other, outer_span);
                return <PixiSpec as toml_span::Deserialize>::deserialize(&mut restored)
                    .map(PackageDependencySpec::Spec);
            }
        };

        let pin_subpackage = table.remove("pin-subpackage");
        let pin_compatible = table.remove("pin-compatible");
        type Constructor = fn(Pin) -> PackageDependencySpec;
        let (key, constructor, mut pin_value): (&str, Constructor, _) =
            match (pin_subpackage, pin_compatible) {
                (Some(_), Some(_)) => {
                    return Err(custom_error(
                        "`pin-subpackage` and `pin-compatible` cannot be combined",
                        outer_span,
                    )
                    .into());
                }
                (Some(value), None) => (
                    "pin-subpackage",
                    PackageDependencySpec::PinSubpackage,
                    value,
                ),
                (None, Some(value)) => (
                    "pin-compatible",
                    PackageDependencySpec::PinCompatible,
                    value,
                ),
                (None, None) => {
                    // Neither pin key: delegate the whole table to `PixiSpec`.
                    let mut table_value = Value::with_span(ValueInner::Table(table), outer_span);
                    return <PixiSpec as toml_span::Deserialize>::deserialize(&mut table_value)
                        .map(PackageDependencySpec::Spec);
                }
            };

        // A pin cannot combine with any other matchspec field.
        if !table.is_empty() {
            let other_key = table.keys().next().map(|k| k.name.to_string());
            return Err(custom_error(
                format!(
                    "`{key}` cannot be combined with other fields (found `{}`)",
                    other_key.unwrap_or_default()
                ),
                outer_span,
            )
            .into());
        }

        let pin = match pin_value.take() {
            // `= true` is the no-argument `pin_compatible(...)` /
            // `pin_subpackage(...)` call: rattler-build's default bounds.
            ValueInner::Boolean(true) => Pin::default(),
            ValueInner::Boolean(false) => {
                return Err(custom_error(
                    format!(
                        "`{key} = false` has no meaning; remove the key or use a table to configure the pin"
                    ),
                    pin_value.span,
                )
                .into());
            }
            inner @ ValueInner::Table(_) => {
                let mut tmp = Value::with_span(inner, pin_value.span);
                <Pin as toml_span::Deserialize>::deserialize(&mut tmp)?
            }
            other => {
                return Err(toml_span::de_helpers::expected(
                    "a boolean or a table",
                    other,
                    pin_value.span,
                )
                .into());
            }
        };

        Ok(constructor(pin))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_spec::{PinBound, PinExpression};

    /// `PackageDependencySpec` deserializes a *value*, but `toml_span::parse`
    /// always returns a table at the top level (a document). Wrap the fixture
    /// in `v = ...` and pull out the inner value to test arbitrary value
    /// shapes (string, table, ...).
    fn parse(input: &str) -> Result<PackageDependencySpec, DeserError> {
        let document = format!("v = {input}");
        let mut value = toml_span::parse(&document).expect("expected valid TOML");
        let ValueInner::Table(mut table) = value.take() else {
            panic!("expected a table");
        };
        let mut entry = table.remove("v").expect("expected a `v` key");
        <PackageDependencySpec as toml_span::Deserialize>::deserialize(&mut entry)
    }

    fn parse_err(input: &str) -> String {
        format!("{:?}", parse(input).expect_err("expected a parse failure"))
    }

    #[test]
    fn parses_plain_string_as_spec() {
        let spec = parse(r#""1.0""#).unwrap();
        assert!(matches!(spec, PackageDependencySpec::Spec(_)));
    }

    #[test]
    fn parses_plain_table_as_spec() {
        let spec = parse(r#"{ version = "1.0", build = "py*" }"#).unwrap();
        assert!(matches!(spec, PackageDependencySpec::Spec(_)));
    }

    #[test]
    fn pin_compatible_true_uses_default_bounds() {
        let spec = parse("{ pin-compatible = true }").unwrap();
        let PackageDependencySpec::PinCompatible(pin) = spec else {
            panic!("expected PinCompatible");
        };
        assert_eq!(pin, Pin::default());
    }

    #[test]
    fn pin_subpackage_true_uses_default_bounds() {
        let spec = parse("{ pin-subpackage = true }").unwrap();
        let PackageDependencySpec::PinSubpackage(pin) = spec else {
            panic!("expected PinSubpackage");
        };
        assert_eq!(pin, Pin::default());
    }

    #[test]
    fn pin_compatible_table_overrides_bounds() {
        let spec = parse(r#"{ pin-compatible = { lower-bound = "x.x" } }"#).unwrap();
        let PackageDependencySpec::PinCompatible(pin) = spec else {
            panic!("expected PinCompatible");
        };
        assert_eq!(
            pin.lower_bound,
            Some(PinBound::Expression(PinExpression::new(2).unwrap()))
        );
        assert_eq!(pin.upper_bound, Pin::default().upper_bound);
    }

    #[test]
    fn pin_exact_form() {
        let spec = parse("{ pin-subpackage = { exact = true } }").unwrap();
        let PackageDependencySpec::PinSubpackage(pin) = spec else {
            panic!("expected PinSubpackage");
        };
        assert!(pin.exact);
        assert_eq!(pin.lower_bound, None);
        assert_eq!(pin.upper_bound, None);
    }

    #[test]
    fn rejects_both_pin_kinds() {
        let err = parse_err("{ pin-subpackage = true, pin-compatible = true }");
        assert!(err.contains("cannot be combined"), "{err}");
    }

    #[test]
    fn rejects_pin_with_other_fields() {
        let err = parse_err(r#"{ pin-compatible = true, version = "1.0" }"#);
        assert!(
            err.contains("cannot be combined with other fields"),
            "{err}"
        );
    }

    #[test]
    fn rejects_pin_false() {
        let err = parse_err("{ pin-compatible = false }");
        assert!(err.contains("has no meaning"), "{err}");
    }

    #[test]
    fn rejects_pin_string_value() {
        let err = parse_err(r#"{ pin-compatible = "x.x" }"#);
        assert!(err.contains("a boolean or a table"), "{err}");
    }
}
