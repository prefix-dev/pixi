//! Pin specs and their resolution.
//!
//! A [`Pin`] is the structural form of `pin_compatible(...)`: it captures the
//! bounds, exactness, and optional build matcher the user (or backend) wrote,
//! independent of any wire format. Resolving a pin requires a `(version,
//! build_string)` pair from the environment the pin refers to; that lookup is
//! the caller's responsibility (see
//! `pixi_command_dispatcher::build::pin_compatible::resolve_pin_compatible`
//! for the build/host-env case). The math here is pure.

use std::{
    cmp::min,
    fmt::{Display, Formatter},
    str::FromStr,
};

use itertools::Itertools;
use pixi_toml::custom_error;
use rattler_conda_types::{
    Version, VersionBumpError, VersionBumpType, VersionSpec,
    version_spec::{LogicalOperator, RangeOperator},
};
use toml_span::{DeserError, Value, de_helpers::TableHelper, value::ValueInner};

use crate::{DetailedSpec, PixiSpec};

/// A pin spec: the structural inputs to `pin_compatible`.
///
/// Mirrors `rattler_build`'s `Pin::apply` semantics (which is what
/// the conda-build / rattler-build ecosystem expects).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pin {
    /// Lower bound of the resolved version range. `None` means no
    /// lower bound is added to the resulting spec.
    pub lower_bound: Option<PinBound>,
    /// Upper bound of the resolved version range. `None` means no
    /// upper bound is added.
    pub upper_bound: Option<PinBound>,
    /// When `true`, the resolved spec pins to `==version` plus the
    /// resolved record's build matcher. Mutually exclusive with
    /// [`Self::build`].
    pub exact: bool,
    /// Optional build-string matcher to layer onto the resolved spec.
    pub build: Option<String>,
}

/// One side of a pin's version range.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PinBound {
    /// A pin expression like `x.x.x`. The number of `x`s controls how
    /// many version segments are kept (lower bound) or bumped (upper).
    Expression(PinExpression),
    /// A literal version that overrides anything the resolved record
    /// would have contributed.
    Version(Version),
}

/// A validated pin expression: only `x` and `.` are allowed, and the
/// segment count is the number of `x`s.
///
/// Examples: `"x"` (segment_count=1), `"x.x.x"` (segment_count=3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinExpression {
    segment_count: usize,
}

impl PinExpression {
    /// Construct a [`PinExpression`] with the given number of `x`
    /// segments. Errors when `segment_count` is `0`, since an empty
    /// pin expression has no meaning.
    pub fn new(segment_count: usize) -> Result<Self, PinError> {
        if segment_count == 0 {
            return Err(PinError::InvalidPinExpression(
                "Pin expression must have at least one segment".to_string(),
            ));
        }
        Ok(PinExpression { segment_count })
    }

    /// Number of `x` segments in this expression.
    pub fn segment_count(&self) -> usize {
        self.segment_count
    }
}

impl FromStr for PinExpression {
    type Err = PinError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.chars().any(|c| c != 'x' && c != '.') {
            return Err(PinError::InvalidPinExpression(format!(
                "Pin expression can only contain 'x' and '.', got: '{s}'"
            )));
        }
        let segment_count = s.chars().filter(|c| *c == 'x').count();
        PinExpression::new(segment_count)
    }
}

impl Display for PinExpression {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            std::iter::repeat_n('x', self.segment_count).format(".")
        )
    }
}

/// Errors raised while validating or applying a [`Pin`].
///
/// Lookup errors (`PackageNotFound` style) live with the caller that
/// owns the lookup, since they're not part of the resolution math.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PinError {
    /// The pin expression contained characters other than `x` and `.`,
    /// or had no `x` segments.
    #[error("Could not parse pin expression: {0}")]
    InvalidPinExpression(String),

    /// Bumping the resolved version to compute an upper bound failed
    /// (e.g. requested more segments than the version provides).
    #[error("Could not increment version: {0}")]
    VersionBump(String),

    /// `Pin::build` and `Pin::exact: true` are mutually exclusive,
    /// matching `rattler_build`'s constraint.
    #[error("Build specifier and exact=True are not supported together")]
    BuildSpecifierWithExact,

    /// The supplied build matcher (or the resolved record's build
    /// string for `exact`) failed to parse as a `StringMatcher`.
    #[error("Failed to parse build string: {0}")]
    BuildStringParse(String),
}

/// The defaults of a no-argument `pin_compatible(...)` / `pin_subpackage(...)`
/// call in rattler-build: lower bound `x.x.x.x.x.x` (pin to the exact resolved
/// version), upper bound `x` (next-major exclusive).
impl Default for Pin {
    fn default() -> Self {
        Pin {
            lower_bound: Some(PinBound::Expression(
                PinExpression::new(6).expect("6 is a valid segment count"),
            )),
            upper_bound: Some(PinBound::Expression(
                PinExpression::new(1).expect("1 is a valid segment count"),
            )),
            exact: false,
            build: None,
        }
    }
}

impl Pin {
    /// Resolve this pin against an environment record's `version` and
    /// `build_string`, producing a [`PixiSpec`] the solver can use.
    ///
    /// The behavior mirrors `rattler_build`'s `Pin::apply`:
    /// - `exact: true` produces `==version` plus the record's build matcher.
    /// - Otherwise lower/upper bounds are applied per [`PinBound`] semantics.
    /// - `build: Some(...)` adds a build-string matcher, but is mutually
    ///   exclusive with `exact: true` ([`PinError::BuildSpecifierWithExact`]).
    pub fn resolve(&self, version: &Version, build_string: &str) -> Result<PixiSpec, PinError> {
        if self.build.is_some() && self.exact {
            return Err(PinError::BuildSpecifierWithExact);
        }

        if self.exact {
            let version_spec = VersionSpec::Exact(
                rattler_conda_types::version_spec::EqualityOperator::Equals,
                version.clone(),
            );
            let build_matcher = build_string
                .parse()
                .map_err(|e| PinError::BuildStringParse(format!("{e}")))?;
            return Ok(PixiSpec::DetailedVersion(Box::new(DetailedSpec {
                version: Some(version_spec),
                build: Some(build_matcher),
                ..Default::default()
            })));
        }

        let mut constraints = Vec::new();

        if let Some(lower_bound) = &self.lower_bound {
            let lower = apply_pin_bound(lower_bound, version, false)?;
            constraints.push(VersionSpec::Range(RangeOperator::GreaterEquals, lower));
        }

        if let Some(upper_bound) = &self.upper_bound {
            let upper = apply_pin_bound(upper_bound, version, true)?;
            constraints.push(VersionSpec::Range(RangeOperator::Less, upper));
        }

        let version_spec = match constraints.len() {
            0 => VersionSpec::Any,
            1 => constraints.into_iter().next().unwrap(),
            _ => VersionSpec::Group(LogicalOperator::And, constraints),
        };

        if let Some(build) = &self.build {
            let build_matcher = build
                .parse()
                .map_err(|e| PinError::BuildStringParse(format!("{e}")))?;
            return Ok(PixiSpec::DetailedVersion(Box::new(DetailedSpec {
                version: Some(version_spec),
                build: Some(build_matcher),
                ..Default::default()
            })));
        }

        Ok(PixiSpec::Version(version_spec))
    }
}

/// Apply a pin bound to a version.
///
/// For [`PinBound::Expression`], extract or increment the requested
/// number of segments. For [`PinBound::Version`], use the literal as-is.
/// `increment=true` bumps the last segment and appends `.0a0` (the
/// upper-bound idiom).
fn apply_pin_bound(
    bound: &PinBound,
    version: &Version,
    increment: bool,
) -> Result<Version, PinError> {
    match bound {
        PinBound::Expression(expr) => {
            if increment {
                increment_version(version, expr.segment_count())
            } else {
                extract_version_segments(version, expr.segment_count())
            }
        }
        PinBound::Version(v) => Ok(v.clone()),
    }
}

/// Truncate `version` to its first `segment_count` segments.
/// Example: `"1.2.3"` with `segment_count=2` -> `"1.2"`.
fn extract_version_segments(version: &Version, segment_count: usize) -> Result<Version, PinError> {
    version
        .clone()
        .with_segments(..min(version.segment_count(), segment_count))
        .ok_or_else(|| {
            PinError::VersionBump(format!(
                "Failed to extract {segment_count} segments from version {version}"
            ))
        })
}

/// Truncate to `segment_count` segments, bump the last segment, and
/// append `.0a0` (with any local version stripped).
///
/// Examples: `"1.2.3"` with `segment_count=2` -> `"1.3.0a0"`,
/// `"1.2.3"` with `segment_count=3` -> `"1.2.4.0a0"`.
fn increment_version(version: &Version, segment_count: usize) -> Result<Version, PinError> {
    if segment_count == 0 {
        return Err(PinError::VersionBump(
            "Segment count must be at least 1".to_string(),
        ));
    }

    let truncated = version
        .clone()
        .with_segments(..min(version.segment_count(), segment_count))
        .ok_or_else(|| {
            PinError::VersionBump(format!(
                "Failed to extract {segment_count} segments from version {version}"
            ))
        })?;

    let bumped = truncated
        .bump(VersionBumpType::Segment((segment_count - 1) as i32))
        .map_err(|e: VersionBumpError| PinError::VersionBump(e.to_string()))?;

    Ok(bumped.with_alpha().remove_local().into_owned())
}

/// Parses a `lower-bound`/`upper-bound` value: either a pin expression
/// (`"x"`, `"x.x"`, ...) or a literal version. Pin expressions are tried
/// first since they only allow `x` and `.`, which never parse as a
/// [`Version`].
impl<'de> toml_span::Deserialize<'de> for PinBound {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let span = value.span;
        let s = match value.take() {
            ValueInner::String(s) => s,
            inner => return Err(toml_span::de_helpers::expected("a string", inner, span).into()),
        };
        if let Ok(expr) = PinExpression::from_str(&s) {
            return Ok(PinBound::Expression(expr));
        }
        match Version::from_str(&s) {
            Ok(version) => Ok(PinBound::Version(version)),
            Err(e) => Err(custom_error(
                format!("'{s}' is not a valid pin expression (e.g. 'x.x') or version: {e}"),
                span,
            )
            .into()),
        }
    }
}

/// TOML form of [`Pin`]:
/// ```toml
/// boltons = { pin-compatible = { lower-bound = "x.x", upper-bound = "x", build = "py*" } }
/// ```
/// Bounds that are not given fall back to the rattler-build defaults
/// ([`Pin::default`]), so an empty table equals a bare
/// `pin_compatible('boltons')` call.
///
/// `exact = true` must be the only key, mirroring rattler-build's jinja
/// functions where `exact=True` rejects every other argument and clears
/// both bounds.
impl<'de> toml_span::Deserialize<'de> for Pin {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let outer_span = value.span;
        let mut th = TableHelper::new(value)?;
        let lower_bound = th.optional("lower-bound");
        let upper_bound = th.optional("upper-bound");
        let exact = th.optional("exact").unwrap_or(false);
        let build: Option<String> = th.optional("build");
        th.finalize(None)?;

        if exact {
            if lower_bound.is_some() || upper_bound.is_some() || build.is_some() {
                return Err(custom_error(
                    "`exact = true` cannot be combined with `lower-bound`, `upper-bound`, or `build`",
                    outer_span,
                )
                .into());
            }
            return Ok(Pin {
                lower_bound: None,
                upper_bound: None,
                exact: true,
                build: None,
            });
        }

        let defaults = Pin::default();
        Ok(Pin {
            lower_bound: lower_bound.or(defaults.lower_bound),
            upper_bound: upper_bound.or(defaults.upper_bound),
            exact: false,
            build,
        })
    }
}

/// Conversions from the `pixi_build_types` wire format. Behind the
/// `pixi_build_types` feature so this crate doesn't pick up the wire
/// crate as a hard dependency.
#[cfg(feature = "pixi_build_types")]
mod build_types_conversions {
    use std::str::FromStr;

    use pixi_build_types as pbt;

    use super::{Pin, PinBound, PinError, PinExpression};

    impl TryFrom<pbt::PinExpression> for PinExpression {
        type Error = PinError;

        fn try_from(value: pbt::PinExpression) -> Result<Self, Self::Error> {
            PinExpression::from_str(&value.0)
        }
    }

    impl TryFrom<pbt::PinBound> for PinBound {
        type Error = PinError;

        fn try_from(value: pbt::PinBound) -> Result<Self, Self::Error> {
            Ok(match value {
                pbt::PinBound::Expression(expr) => PinBound::Expression(expr.try_into()?),
                pbt::PinBound::Version(v) => PinBound::Version(v),
            })
        }
    }

    impl TryFrom<pbt::PinCompatibleSpec> for Pin {
        type Error = PinError;

        fn try_from(value: pbt::PinCompatibleSpec) -> Result<Self, Self::Error> {
            Ok(Pin {
                lower_bound: value.lower_bound.map(PinBound::try_from).transpose()?,
                upper_bound: value.upper_bound.map(PinBound::try_from).transpose()?,
                exact: value.exact,
                build: value.build,
            })
        }
    }

    impl From<&PinExpression> for pbt::PinExpression {
        fn from(value: &PinExpression) -> Self {
            pbt::PinExpression(value.to_string())
        }
    }

    impl From<&PinBound> for pbt::PinBound {
        fn from(value: &PinBound) -> Self {
            match value {
                PinBound::Expression(expr) => pbt::PinBound::Expression(expr.into()),
                PinBound::Version(v) => pbt::PinBound::Version(v.clone()),
            }
        }
    }

    /// Inverse of `TryFrom<pbt::PinCompatibleSpec> for Pin`. This direction is
    /// infallible: every [`Pin`] can be represented on the wire.
    impl From<&Pin> for pbt::PinCompatibleSpec {
        fn from(value: &Pin) -> Self {
            pbt::PinCompatibleSpec {
                lower_bound: value.lower_bound.as_ref().map(pbt::PinBound::from),
                upper_bound: value.upper_bound.as_ref().map(pbt::PinBound::from),
                exact: value.exact,
                build: value.build.clone(),
            }
        }
    }

    impl TryFrom<pbt::PinSubpackageSpec> for Pin {
        type Error = PinError;

        fn try_from(value: pbt::PinSubpackageSpec) -> Result<Self, Self::Error> {
            Ok(Pin {
                lower_bound: value.lower_bound.map(PinBound::try_from).transpose()?,
                upper_bound: value.upper_bound.map(PinBound::try_from).transpose()?,
                exact: value.exact,
                build: value.build,
            })
        }
    }

    impl From<&Pin> for pbt::PinSubpackageSpec {
        fn from(value: &Pin) -> Self {
            pbt::PinSubpackageSpec {
                lower_bound: value.lower_bound.as_ref().map(pbt::PinBound::from),
                upper_bound: value.upper_bound.as_ref().map(pbt::PinBound::from),
                exact: value.exact,
                build: value.build.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_expression_parses_xs_and_dots() {
        assert!(PinExpression::from_str("x").is_ok());
        assert!(PinExpression::from_str("x.x.x").is_ok());
        assert_eq!(PinExpression::from_str("x.x.x").unwrap().segment_count(), 3);
        assert!(PinExpression::from_str("foo").is_err());
        assert!(PinExpression::from_str("").is_err());
    }

    #[test]
    fn pin_expression_roundtrips_via_display() {
        assert_eq!(PinExpression::new(3).unwrap().to_string(), "x.x.x");
    }

    #[test]
    fn resolve_exact_emits_equals_with_build_matcher() {
        let pin = Pin {
            lower_bound: None,
            upper_bound: None,
            exact: true,
            build: None,
        };
        let v = Version::from_str("3.11.5").unwrap();
        let spec = pin.resolve(&v, "h12345").unwrap();
        let PixiSpec::DetailedVersion(detailed) = spec else {
            panic!("expected Detailed");
        };
        assert_eq!(detailed.version.unwrap().to_string(), "==3.11.5");
        assert!(detailed.build.is_some());
    }

    #[test]
    fn resolve_lower_and_upper_bounds_via_expression() {
        let pin = Pin {
            lower_bound: Some(PinBound::Expression(PinExpression::new(2).unwrap())),
            upper_bound: Some(PinBound::Expression(PinExpression::new(2).unwrap())),
            exact: false,
            build: None,
        };
        let v = Version::from_str("3.11.5").unwrap();
        let spec = pin.resolve(&v, "h12345").unwrap();
        let PixiSpec::Version(vs) = spec else {
            panic!("expected Version");
        };
        assert_eq!(vs.to_string(), ">=3.11,<3.12.0a0");
    }

    #[test]
    fn resolve_no_bounds_emits_any() {
        let pin = Pin {
            lower_bound: None,
            upper_bound: None,
            exact: false,
            build: None,
        };
        let v = Version::from_str("1.0.0").unwrap();
        let spec = pin.resolve(&v, "h0").unwrap();
        assert!(matches!(spec, PixiSpec::Version(VersionSpec::Any)));
    }

    /// `Pin` deserializes a *value*, but `toml_span::parse` always returns a
    /// table at the top level. Wrap the fixture in `v = ...` and pull out the
    /// inner value to test arbitrary value shapes.
    fn parse_pin(input: &str) -> Result<Pin, toml_span::DeserError> {
        let document = format!("v = {input}");
        let mut value = toml_span::parse(&document).expect("valid TOML");
        let toml_span::value::ValueInner::Table(mut table) = value.take() else {
            panic!("expected a table");
        };
        let mut entry = table.remove("v").expect("expected a `v` key");
        <Pin as toml_span::Deserialize>::deserialize(&mut entry)
    }

    #[test]
    fn toml_empty_table_uses_rattler_build_defaults() {
        let pin = parse_pin("{}").unwrap();
        assert_eq!(pin, Pin::default());
        let v = Version::from_str("2.0.1").unwrap();
        let PixiSpec::Version(vs) = pin.resolve(&v, "h123").unwrap() else {
            panic!("expected Version");
        };
        assert_eq!(vs.to_string(), ">=2.0.1,<3.0a0");
    }

    #[test]
    fn toml_single_bound_keeps_other_default() {
        let pin = parse_pin(r#"{ lower-bound = "x.x" }"#).unwrap();
        assert_eq!(
            pin.lower_bound,
            Some(PinBound::Expression(PinExpression::new(2).unwrap()))
        );
        assert_eq!(pin.upper_bound, Pin::default().upper_bound);
    }

    #[test]
    fn toml_build_keeps_default_bounds() {
        let pin = parse_pin(r#"{ build = "mpi_mpich_*" }"#).unwrap();
        assert_eq!(pin.lower_bound, Pin::default().lower_bound);
        assert_eq!(pin.upper_bound, Pin::default().upper_bound);
        assert_eq!(pin.build.as_deref(), Some("mpi_mpich_*"));
    }

    #[test]
    fn toml_exact_clears_bounds() {
        let pin = parse_pin("{ exact = true }").unwrap();
        assert_eq!(
            pin,
            Pin {
                lower_bound: None,
                upper_bound: None,
                exact: true,
                build: None,
            }
        );
    }

    #[test]
    fn toml_exact_rejects_other_keys() {
        let err = parse_pin(r#"{ exact = true, lower-bound = "x.x" }"#).unwrap_err();
        assert!(format!("{err:?}").contains("cannot be combined"));
    }

    #[test]
    fn toml_literal_version_bound() {
        let pin = parse_pin(r#"{ upper-bound = "9.9" }"#).unwrap();
        assert_eq!(
            pin.upper_bound,
            Some(PinBound::Version(Version::from_str("9.9").unwrap()))
        );
    }

    #[test]
    fn toml_invalid_bound_is_rejected() {
        let err = parse_pin(r#"{ lower-bound = "not-a-version!" }"#).unwrap_err();
        assert!(format!("{err:?}").contains("not a valid pin expression"));
    }

    #[test]
    fn toml_unknown_key_is_rejected() {
        assert!(parse_pin(r#"{ lowerbound = "x.x" }"#).is_err());
    }

    #[test]
    fn resolve_rejects_build_with_exact() {
        let pin = Pin {
            lower_bound: None,
            upper_bound: None,
            exact: true,
            build: Some("h*".to_string()),
        };
        let v = Version::from_str("1.0.0").unwrap();
        assert!(matches!(
            pin.resolve(&v, "h0"),
            Err(PinError::BuildSpecifierWithExact)
        ));
    }
}
