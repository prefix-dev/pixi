//! Pin compatible resolution for build dependencies
//!
//! This module implements the `pin_compatible` functionality from rattler_build,
//! which allows runtime dependencies to be pinned based on resolved versions
//! from build/host environments.

use itertools::Itertools;
use pixi_build_types::{PinBound, PinCompatibleSpec};
use pixi_record::PixiRecord;
use pixi_spec::{DetailedSpec, PixiSpec};
use rattler_conda_types::version_spec::{LogicalOperator, RangeOperator};
use rattler_conda_types::{PackageName, Version, VersionBumpError, VersionBumpType, VersionSpec};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// A map of resolved packages that can be referenced by pin_compatible
pub type PinCompatibilityMap<'a> = HashMap<PackageName, &'a PixiRecord>;

/// A validated pin expression that can only contain 'x' and '.'
///
/// Just stores the segment count - can reconstruct the string if needed.
/// Examples: segment_count=1 → "x", segment_count=3 → "x.x.x"
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinExpression {
    /// The number of 'x' segments in the expression
    segment_count: usize,
}

impl PinExpression {
    /// Create a new pin expression with the given segment count
    pub fn new(segment_count: usize) -> Result<Self, PinCompatibleError> {
        if segment_count == 0 {
            return Err(PinCompatibleError::InvalidPinExpression(
                "Pin expression must have at least one segment".to_string(),
            ));
        }
        Ok(PinExpression { segment_count })
    }

    /// Get the number of segments (number of 'x' characters)
    pub fn segment_count(&self) -> usize {
        self.segment_count
    }
}

impl FromStr for PinExpression {
    type Err = PinCompatibleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Validate that string only contains 'x' and '.'
        if s.chars().any(|c| c != 'x' && c != '.') {
            return Err(PinCompatibleError::InvalidPinExpression(format!(
                "Pin expression can only contain 'x' and '.', got: '{}'",
                s
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

#[derive(Debug, Clone, thiserror::Error)]
pub enum PinCompatibleError {
    #[error("Could not apply pin_compatible. Package '{}' is not in the compatibility environment", .0.as_normalized())]
    PackageNotFound(PackageName),

    #[error("Could not parse pin expression: {0}")]
    InvalidPinExpression(String),

    #[error("Could not increment version: {0}")]
    VersionBump(String),

    #[error("Build specifier and exact=True are not supported together")]
    BuildSpecifierWithExact,

    #[error("Failed to parse build string: {0}")]
    BuildStringParse(String),
}

/// Resolve a pin_compatible spec against solved environment records
///
/// Mimics rattler_build's Pin::apply logic but returns a PixiSpec
pub fn resolve_pin_compatible(
    package_name: &PackageName,
    spec: &PinCompatibleSpec,
    compatibility_map: &PinCompatibilityMap<'_>,
) -> Result<PixiSpec, PinCompatibleError> {
    // 1. Find the package in the compatibility map (O(1) lookup)
    let record = compatibility_map
        .get(package_name)
        .ok_or_else(|| PinCompatibleError::PackageNotFound(package_name.clone()))?;

    let version = &record.package_record().version;
    let build_string = &record.package_record().build;

    // 2. Check for conflicting args
    if spec.build.is_some() && spec.exact {
        return Err(PinCompatibleError::BuildSpecifierWithExact);
    }

    // 3. Handle exact pin
    if spec.exact {
        let version_spec = VersionSpec::Exact(
            rattler_conda_types::version_spec::EqualityOperator::Equals,
            version.clone().into(),
        );
        let build_matcher = build_string
            .parse()
            .map_err(|e| PinCompatibleError::BuildStringParse(format!("{}", e)))?;

        return Ok(PixiSpec::DetailedVersion(Box::new(DetailedSpec {
            version: Some(version_spec),
            build: Some(build_matcher),
            ..Default::default()
        })));
    }

    // 4. Build version constraints from bounds using VersionSpec types directly
    let mut constraints = Vec::new();

    // Lower bound: >=version
    if let Some(lower_bound) = &spec.lower_bound {
        let lower = apply_pin_bound(lower_bound, version, false)?;
        constraints.push(VersionSpec::Range(RangeOperator::GreaterEquals, lower));
    }

    // Upper bound: <version
    if let Some(upper_bound) = &spec.upper_bound {
        let upper = apply_pin_bound(upper_bound, version, true)?;
        constraints.push(VersionSpec::Range(RangeOperator::Less, upper));
    }

    // 5. Construct VersionSpec (combine with AND if multiple constraints)
    let version_spec = match constraints.len() {
        0 => VersionSpec::Any,
        1 => constraints.into_iter().next().unwrap(),
        _ => VersionSpec::Group(LogicalOperator::And, constraints),
    };

    // 6. Add build matcher if specified
    if let Some(build) = &spec.build {
        let build_matcher = build
            .parse()
            .map_err(|e| PinCompatibleError::BuildStringParse(format!("{}", e)))?;

        return Ok(PixiSpec::DetailedVersion(Box::new(DetailedSpec {
            version: Some(version_spec),
            build: Some(build_matcher),
            ..Default::default()
        })));
    }

    // 7. Return simple version spec
    Ok(PixiSpec::Version(version_spec))
}

/// Apply a pin bound to a version
///
/// - For Expression: extract N segments from version or increment
/// - For Version: use as-is
/// - If increment=true: bump the last segment and add .0a0
fn apply_pin_bound(
    bound: &PinBound,
    version: &Version,
    increment: bool,
) -> Result<Version, PinCompatibleError> {
    match bound {
        PinBound::Expression(pin_expr) => {
            // Parse and validate the expression string
            let expr = PinExpression::from_str(&pin_expr.0)?;

            if increment {
                // Increment version (like rattler_build's increment function)
                increment_version(version, expr.segment_count())
            } else {
                // Extract segments for lower bound
                extract_version_segments(version, expr.segment_count())
            }
        }
        PinBound::Version(v) => Ok(v.clone()),
    }
}

/// Extract N segments from a version (for lower bound)
///
/// Example: "1.2.3" with segment_count=2 → "1.2"
fn extract_version_segments(
    version: &Version,
    segment_count: usize,
) -> Result<Version, PinCompatibleError> {
    use std::cmp::min;

    // Extract only the first N segments
    version
        .clone()
        .with_segments(..min(version.segment_count(), segment_count))
        .ok_or_else(|| {
            PinCompatibleError::VersionBump(format!(
                "Failed to extract {} segments from version {}",
                segment_count, version
            ))
        })
}

/// Increment a version at the Nth segment (for upper bound)
///
/// Example: "1.2.3" with segment_count=2 → "1.3.0a0"
/// Example: "1.2.3" with segment_count=3 → "1.2.4.0a0"
///
/// This mimics rattler_build's increment() function
fn increment_version(
    version: &Version,
    segment_count: usize,
) -> Result<Version, PinCompatibleError> {
    use std::cmp::min;

    if segment_count == 0 {
        return Err(PinCompatibleError::VersionBump(
            "Segment count must be at least 1".to_string(),
        ));
    }

    // Extract first N segments
    let truncated = version
        .clone()
        .with_segments(..min(version.segment_count(), segment_count))
        .ok_or_else(|| {
            PinCompatibleError::VersionBump(format!(
                "Failed to extract {} segments from version {}",
                segment_count, version
            ))
        })?;

    // Bump the last segment (segment_count - 1)
    let bumped = truncated
        .bump(VersionBumpType::Segment((segment_count - 1) as i32))
        .map_err(|e: VersionBumpError| PinCompatibleError::VersionBump(e.to_string()))?;

    // Add .0a0 suffix and remove local version if present
    Ok(bumped.with_alpha().remove_local().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // PinExpression tests
    #[test]
    fn test_pin_expression_valid() {
        assert!(PinExpression::from_str("x").is_ok());
        assert!(PinExpression::from_str("x.x").is_ok());
        assert!(PinExpression::from_str("x.x.x").is_ok());
        assert!(PinExpression::from_str("x.x.x.x.x.x").is_ok());

        let expr = PinExpression::from_str("x.x.x").unwrap();
        assert_eq!(expr.segment_count(), 3);
        assert_eq!(expr.to_string(), "x.x.x");
    }

    #[test]
    fn test_pin_expression_new() {
        let expr = PinExpression::new(3).unwrap();
        assert_eq!(expr.segment_count(), 3);
        assert_eq!(expr.to_string(), "x.x.x");

        assert!(PinExpression::new(0).is_err());
    }

    #[test]
    fn test_pin_expression_invalid() {
        // Contains invalid characters
        assert!(PinExpression::from_str("x.y").is_err());
        assert!(PinExpression::from_str("1.2.3").is_err());
        assert!(PinExpression::from_str("x.x.x.4").is_err());

        // Empty or no 'x'
        assert!(PinExpression::from_str("").is_err());
        assert!(PinExpression::from_str("...").is_err());
    }

    // Version manipulation tests
    #[test]
    fn test_extract_version_segments() {
        let version = Version::from_str("1.2.3").unwrap();

        assert_eq!(
            extract_version_segments(&version, 1).unwrap().to_string(),
            "1"
        );
        assert_eq!(
            extract_version_segments(&version, 2).unwrap().to_string(),
            "1.2"
        );
        assert_eq!(
            extract_version_segments(&version, 3).unwrap().to_string(),
            "1.2.3"
        );
        // More segments than version has should just return full version
        assert_eq!(
            extract_version_segments(&version, 5).unwrap().to_string(),
            "1.2.3"
        );
    }

    #[test]
    fn test_increment_version() {
        // Test basic increment
        let version = Version::from_str("1.2.3").unwrap();

        // Increment at segment 1: 1 -> 2.0a0
        assert_eq!(increment_version(&version, 1).unwrap().to_string(), "2.0a0");

        // Increment at segment 2: 1.2 -> 1.3.0a0
        assert_eq!(
            increment_version(&version, 2).unwrap().to_string(),
            "1.3.0a0"
        );

        // Increment at segment 3: 1.2.3 -> 1.2.4.0a0
        assert_eq!(
            increment_version(&version, 3).unwrap().to_string(),
            "1.2.4.0a0"
        );

        // Increment beyond version length: uses actual segments + pads
        // 1.2.3 with 5 segments: truncate to 3, then bump segment 4 (5-1=4, but max is 2)
        // This creates 1.2.3.0.1.0a0 which is the rattler behavior
        assert_eq!(
            increment_version(&version, 5).unwrap().to_string(),
            "1.2.3.0.1.0a0"
        );
    }

    #[test]
    fn test_increment_version_with_local() {
        // Version with local part should have it removed
        let version = Version::from_str("1.2.3+local").unwrap();
        assert_eq!(
            increment_version(&version, 2).unwrap().to_string(),
            "1.3.0a0"
        );
    }

    #[test]
    fn test_increment_version_zero_segments() {
        let version = Version::from_str("1.2.3").unwrap();
        assert!(increment_version(&version, 0).is_err());
    }

    // Helper to create a test PixiRecord
    fn create_test_record(name: &str, version: &str, build: &str) -> PixiRecord {
        use rattler_conda_types::{NoArchType, PackageRecord, Platform, RepoDataRecord};
        use std::collections::BTreeMap;
        use url::Url;

        let package_record = PackageRecord {
            arch: None,
            build: build.to_string(),
            build_number: 0,
            constrains: vec![],
            depends: vec![],
            features: None,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            md5: None,
            name: PackageName::new_unchecked(name),
            noarch: NoArchType::default(),
            platform: Some(Platform::Linux64.to_string()),
            sha256: None,
            size: None,
            subdir: "linux-64".to_string(),
            timestamp: None,
            track_features: vec![],
            version: Version::from_str(version).unwrap().into(),
            purls: None,
            run_exports: None,
            experimental_extra_depends: BTreeMap::new(),
            python_site_packages_path: None,
        };

        PixiRecord::Binary(RepoDataRecord {
            package_record,
            file_name: format!("{}-{}-{}.conda", name, version, build),
            url: Url::parse("https://conda.anaconda.org/conda-forge/linux-64/test.conda").unwrap(),
            channel: Some("conda-forge".to_string()),
        })
    }

    // Pin compatible resolution tests
    #[test]
    fn test_pin_compatible_basic_bounds() {
        // Setup: python 3.11.5
        let python_record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: pin_compatible("python", lower_bound="x.x", upper_bound="x.x")
        // Expected: >=3.11,<3.12.0a0
        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            upper_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map).unwrap();

        // Verify it's a Version spec
        if let PixiSpec::Version(version_spec) = result {
            assert_eq!(version_spec.to_string(), ">=3.11,<3.12.0a0");
        } else {
            panic!("Expected PixiSpec::Version");
        }
    }

    #[test]
    fn test_pin_compatible_three_segments() {
        // Setup: numpy 1.23.4
        let numpy_record = create_test_record("numpy", "1.23.4", "py311h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("numpy"), &numpy_record);

        // Test: pin_compatible("numpy", lower_bound="x.x.x", upper_bound="x.x.x")
        // Expected: >=1.23.4,<1.23.5.0a0
        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x.x".to_string(),
            ))),
            upper_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x.x".to_string(),
            ))),
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("numpy"), &spec, &map).unwrap();

        if let PixiSpec::Version(version_spec) = result {
            assert_eq!(version_spec.to_string(), ">=1.23.4,<1.23.5.0a0");
        } else {
            panic!("Expected PixiSpec::Version");
        }
    }

    #[test]
    fn test_pin_compatible_major_only() {
        // Setup: python 3.11.5
        let python_record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: pin_compatible("python", lower_bound="x", upper_bound="x")
        // Expected: >=3,<4.0a0
        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x".to_string(),
            ))),
            upper_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x".to_string(),
            ))),
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map).unwrap();

        if let PixiSpec::Version(version_spec) = result {
            assert_eq!(version_spec.to_string(), ">=3,<4.0a0");
        } else {
            panic!("Expected PixiSpec::Version");
        }
    }

    #[test]
    fn test_pin_compatible_lower_bound_only() {
        // Setup: openssl 1.1.1k
        let openssl_record = create_test_record("openssl", "1.1.1", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("openssl"), &openssl_record);

        // Test: pin_compatible("openssl", lower_bound="x.x.x", upper_bound=None)
        // Expected: >=1.1.1
        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x.x".to_string(),
            ))),
            upper_bound: None,
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("openssl"), &spec, &map).unwrap();

        if let PixiSpec::Version(version_spec) = result {
            assert_eq!(version_spec.to_string(), ">=1.1.1");
        } else {
            panic!("Expected PixiSpec::Version");
        }
    }

    #[test]
    fn test_pin_compatible_upper_bound_only() {
        // Setup: openssl 1.1.1k
        let openssl_record = create_test_record("openssl", "1.1.1", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("openssl"), &openssl_record);

        // Test: pin_compatible("openssl", lower_bound=None, upper_bound="x.x")
        // Expected: <1.2.0a0
        let spec = PinCompatibleSpec {
            lower_bound: None,
            upper_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("openssl"), &spec, &map).unwrap();

        if let PixiSpec::Version(version_spec) = result {
            assert_eq!(version_spec.to_string(), "<1.2.0a0");
        } else {
            panic!("Expected PixiSpec::Version");
        }
    }

    #[test]
    fn test_pin_compatible_exact() {
        // Setup: python 3.11.5
        let python_record = create_test_record("python", "3.11.5", "h12345_0");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: pin_compatible("python", exact=True)
        // Expected: ==3.11.5 h12345_0
        let spec = PinCompatibleSpec {
            lower_bound: None,
            upper_bound: None,
            exact: true,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map).unwrap();

        if let PixiSpec::DetailedVersion(detailed) = result {
            assert_eq!(detailed.version.as_ref().unwrap().to_string(), "==3.11.5");
            assert_eq!(detailed.build.as_ref().unwrap().to_string(), "h12345_0");
        } else {
            panic!("Expected PixiSpec::DetailedVersion");
        }
    }

    #[test]
    fn test_pin_compatible_with_build_string() {
        // Setup: python 3.11.5
        let python_record = create_test_record("python", "3.11.5", "h12345_0");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: pin_compatible("python", lower_bound="x.x", upper_bound="x.x", build="h*")
        // Expected: >=3.11,<3.12.0a0 h*
        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            upper_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            exact: false,
            build: Some("h*".to_string()),
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map).unwrap();

        if let PixiSpec::DetailedVersion(detailed) = result {
            assert_eq!(
                detailed.version.as_ref().unwrap().to_string(),
                ">=3.11,<3.12.0a0"
            );
            assert_eq!(detailed.build.as_ref().unwrap().to_string(), "h*");
        } else {
            panic!("Expected PixiSpec::DetailedVersion");
        }
    }

    #[test]
    fn test_pin_compatible_literal_version_bounds() {
        // Setup: python 3.11.5
        let python_record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: pin_compatible("python", lower_bound="3.10", upper_bound="3.12")
        // Expected: >=3.10,<3.12
        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Version(Version::from_str("3.10").unwrap())),
            upper_bound: Some(PinBound::Version(Version::from_str("3.12").unwrap())),
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map).unwrap();

        if let PixiSpec::Version(version_spec) = result {
            assert_eq!(version_spec.to_string(), ">=3.10,<3.12");
        } else {
            panic!("Expected PixiSpec::Version");
        }
    }

    #[test]
    fn test_pin_compatible_no_bounds() {
        // Setup: python 3.11.5
        let python_record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: pin_compatible("python") with no bounds
        // Expected: * (any version)
        let spec = PinCompatibleSpec {
            lower_bound: None,
            upper_bound: None,
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map).unwrap();

        if let PixiSpec::Version(version_spec) = result {
            assert_eq!(version_spec, VersionSpec::Any);
        } else {
            panic!("Expected PixiSpec::Version");
        }
    }

    // Error cases
    #[test]
    fn test_pin_compatible_package_not_found() {
        let map = PinCompatibilityMap::new();

        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            upper_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            exact: false,
            build: None,
        };

        let result =
            resolve_pin_compatible(&PackageName::new_unchecked("nonexistent"), &spec, &map);
        assert!(matches!(
            result,
            Err(PinCompatibleError::PackageNotFound(_))
        ));
    }

    #[test]
    fn test_pin_compatible_exact_with_build_error() {
        let python_record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: exact=true with build should error
        let spec = PinCompatibleSpec {
            lower_bound: None,
            upper_bound: None,
            exact: true,
            build: Some("h*".to_string()),
        };

        let result = resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map);
        assert!(matches!(
            result,
            Err(PinCompatibleError::BuildSpecifierWithExact)
        ));
    }

    #[test]
    fn test_pin_compatible_invalid_expression() {
        let python_record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: invalid expression
        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.y.z".to_string(),
            ))),
            upper_bound: None,
            exact: false,
            build: None,
        };

        let result = resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map);
        assert!(matches!(
            result,
            Err(PinCompatibleError::InvalidPinExpression(_))
        ));
    }

    #[test]
    fn test_pin_compatible_invalid_build_string() {
        let python_record = create_test_record("python", "3.11.5", "h12345");
        let mut map = PinCompatibilityMap::new();
        map.insert(PackageName::new_unchecked("python"), &python_record);

        // Test: invalid build string (use a pattern that glob parsing will reject)
        let spec = PinCompatibleSpec {
            lower_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            upper_bound: Some(PinBound::Expression(pixi_build_types::PinExpression(
                "x.x".to_string(),
            ))),
            exact: false,
            build: Some("**[".to_string()), // Invalid glob - unterminated character class
        };

        let result = resolve_pin_compatible(&PackageName::new_unchecked("python"), &spec, &map);
        assert!(matches!(
            result,
            Err(PinCompatibleError::BuildStringParse(_))
        ));
    }
}
