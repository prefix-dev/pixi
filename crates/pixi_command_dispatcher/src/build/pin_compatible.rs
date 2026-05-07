//! Pin-compatible resolution for build dependencies.
//!
//! The structural [`Pin`] type and its resolution math live in
//! `pixi_spec`. This module owns the lookup-by-name layer: given a
//! `PinCompatibleSpec` from the backend protocol and a
//! [`PinCompatibilityMap`] of solved records, find the named record
//! and apply the pin against its `(version, build_string)` pair.

use std::collections::HashMap;

use pixi_build_types::PinCompatibleSpec;
use pixi_record::PixiRecord;
use pixi_spec::{Pin, PinError, PixiSpec};
use rattler_conda_types::PackageName;

/// A map of resolved packages that can be referenced by `pin_compatible`.
pub type PinCompatibilityMap<'a> = HashMap<PackageName, &'a PixiRecord>;

/// Errors raised while resolving a `pin_compatible` reference.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PinCompatibleError {
    /// The pin references a package that the compatibility map doesn't
    /// contain. For host deps this means the build env never resolved
    /// the package; for run deps it means neither build nor host did.
    #[error(
        "Could not apply pin_compatible. Package '{}' is not in the compatibility environment",
        .0.as_normalized()
    )]
    PackageNotFound(PackageName),

    /// The pin's structural shape was invalid, or the resolution math
    /// failed (version bump, build-string parse, etc.).
    #[error(transparent)]
    Pin(#[from] PinError),
}

/// Resolve a `pin_compatible` spec against solved environment records.
///
/// Mimics `rattler_build`'s `Pin::apply` semantics: the lookup half
/// happens here, the math half in [`pixi_spec::Pin::resolve`].
pub fn resolve_pin_compatible(
    package_name: &PackageName,
    spec: &PinCompatibleSpec,
    compatibility_map: &PinCompatibilityMap<'_>,
) -> Result<PixiSpec, PinCompatibleError> {
    let record = compatibility_map
        .get(package_name)
        .ok_or_else(|| PinCompatibleError::PackageNotFound(package_name.clone()))?;
    let pin = Pin::try_from(spec.clone())?;
    Ok(pin.resolve(
        &record.package_record().version,
        &record.package_record().build,
    )?)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pixi_build_types::{PinBound, PinExpression};
    use pixi_record::PixiRecord;
    use rattler_conda_types::{
        PackageName, PackageRecord, RepoDataRecord, VersionWithSource,
        package::DistArchiveIdentifier,
    };
    use std::str::FromStr;
    use url::Url;

    use super::*;

    fn create_test_record(name: &str, version: &str, build: &str) -> PixiRecord {
        let pkg_name = PackageName::new_unchecked(name);
        let mut pr = PackageRecord::new(
            pkg_name,
            VersionWithSource::from_str(version).expect("valid version"),
            build.to_string(),
        );
        pr.subdir = "linux-64".into();
        let file_name = format!("{name}-{version}-{build}.conda");
        PixiRecord::Binary(Arc::new(RepoDataRecord {
            package_record: pr,
            identifier: DistArchiveIdentifier::from_str(&file_name)
                .expect("valid dist archive identifier"),
            url: Url::parse(&format!(
                "https://example.com/conda-forge/linux-64/{file_name}"
            ))
            .expect("valid url"),
            channel: Some("https://example.com/conda-forge".to_string()),
        }))
    }

    #[test]
    fn missing_package_surfaces_as_not_found() {
        let map = PinCompatibilityMap::new();
        let spec = PinCompatibleSpec {
            lower_bound: None,
            upper_bound: None,
            exact: false,
            build: None,
        };
        let err = resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map)
            .expect_err("empty map must surface as PackageNotFound");
        assert!(matches!(err, PinCompatibleError::PackageNotFound(_)));
    }

    #[test]
    fn resolves_via_lookup_then_pin_resolve() {
        let record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &record);

        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(PinExpression("x.x".to_string()))),
            upper_bound: Some(PinBound::Expression(PinExpression("x.x".to_string()))),
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map).unwrap();
        let PixiSpec::Version(vs) = result else {
            panic!("expected Version");
        };
        assert_eq!(vs.to_string(), ">=3.11,<3.12.0a0");
    }

    #[test]
    fn invalid_pin_expression_surfaces_as_pin_error() {
        let record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &record);

        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(PinExpression("foo".to_string()))),
            upper_bound: None,
            exact: false,
            build: None,
        };
        let err = resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map)
            .expect_err("invalid pin expression must surface");
        assert!(matches!(
            err,
            PinCompatibleError::Pin(PinError::InvalidPinExpression(_))
        ));
    }
}
