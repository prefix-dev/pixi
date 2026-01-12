//! Package specification traits
//!
//! # Key components
//!
//! * [`PackageSpec`] - Core trait for package specification behavior
//! * [`AnyVersion`] - Trait for creating wildcard version specifications that
//!   can match any version
//! * [`BinarySpecExt`] - Extension for converting binary specs to nameless
//!   match specs

use std::{fmt::Debug, sync::Arc};

use pixi_build_types::{self as pbt};
use rattler_conda_types::{Channel, MatchSpec, NamelessMatchSpec, PackageName, PackageNameMatcher};

/// Get the * version for the version type, that is currently being used
pub trait AnyVersion {
    /// Get the * version for the version type, that is currently being used
    fn any() -> Self;
}

/// Convert a binary spec to a nameless match spec
pub trait BinarySpecExt {
    /// Return a NamelessMatchSpec from the binary spec
    fn to_nameless(&self) -> NamelessMatchSpec;
}

/// A trait that define the package spec interface
pub trait PackageSpec: Send {
    /// Source representation of a package
    type SourceSpec: PackageSourceSpec;

    /// Returns true if the specified [`PackageSpec`] is a valid variant spec.
    fn can_be_used_as_variant(&self) -> bool;

    /// Converts the package spec to a match spec.
    fn to_match_spec(
        &self,
        name: PackageName,
    ) -> miette::Result<(MatchSpec, Option<Self::SourceSpec>)>;
}

/// A trait that defines the package source spec interface
pub trait PackageSourceSpec: Debug + Send {
    /// Convert this instance into a v1 instance.
    fn to_v1(self) -> pbt::SourcePackageSpec;
}

impl PackageSpec for pbt::PackageSpec {
    type SourceSpec = pbt::SourcePackageSpec;

    fn can_be_used_as_variant(&self) -> bool {
        match self {
            pbt::PackageSpec::Binary(spec) => {
                let pbt::BinaryPackageSpec {
                    version,
                    build,
                    build_number,
                    file_name,
                    channel,
                    subdir,
                    md5,
                    sha256,
                    url,
                    license,
                } = spec;

                version == &Some(rattler_conda_types::VersionSpec::Any)
                    && build.is_none()
                    && build_number.is_none()
                    && file_name.is_none()
                    && channel.is_none()
                    && subdir.is_none()
                    && md5.is_none()
                    && sha256.is_none()
                    && url.is_none()
                    && license.is_none()
            }
            _ => false,
        }
    }

    fn to_match_spec(
        &self,
        name: PackageName,
    ) -> miette::Result<(MatchSpec, Option<Self::SourceSpec>)> {
        match self {
            pbt::PackageSpec::Binary(binary_spec) => {
                // Always use to_nameless() to preserve all fields including build constraints
                let match_spec = MatchSpec::from_nameless(
                    binary_spec.to_nameless(),
                    Some(PackageNameMatcher::Exact(name)),
                );
                Ok((match_spec, None))
            }
            pbt::PackageSpec::Source(source_spec) => Ok((
                MatchSpec {
                    name: Some(PackageNameMatcher::Exact(name)),
                    ..MatchSpec::default()
                },
                Some(source_spec.clone()),
            )),
            pbt::PackageSpec::PinCompatible(_) => {
                miette::bail!("PinCompatible package specs are not yet supported in this context")
            }
        }
    }
}

impl AnyVersion for pbt::PackageSpec {
    fn any() -> Self {
        pbt::PackageSpec::Binary(rattler_conda_types::VersionSpec::Any.into())
    }
}

impl BinarySpecExt for pbt::BinaryPackageSpec {
    fn to_nameless(&self) -> NamelessMatchSpec {
        NamelessMatchSpec {
            version: self.version.clone(),
            build: self.build.clone(),
            build_number: self.build_number.clone(),
            file_name: self.file_name.clone(),
            channel: self
                .channel
                .as_ref()
                .map(|url| Arc::new(Channel::from_url(url.clone()))),
            subdir: self.subdir.clone(),
            md5: self.md5,
            sha256: self.sha256,
            url: self.url.clone(),
            license: self.license.clone(),
            extras: None,
            namespace: None,
            condition: None,
        }
    }
}

impl PackageSourceSpec for pbt::SourcePackageSpec {
    fn to_v1(self) -> pbt::SourcePackageSpec {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::{ParseStrictness, StringMatcher, VersionSpec};

    #[test]
    fn test_to_match_spec_preserves_build_constraint_with_wildcard_version() {
        // Test case: dependency with wildcard version and build constraint
        // e.g., tk = { build = "xft*" }
        let build_matcher: StringMatcher = "xft*".parse().unwrap();
        let binary_spec = pbt::BinaryPackageSpec {
            version: Some(VersionSpec::Any),
            build: Some(build_matcher.clone()),
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            md5: None,
            sha256: None,
            url: None,
            license: None,
        };

        let package_spec = pbt::PackageSpec::Binary(binary_spec);
        let package_name = PackageName::try_from("tk").unwrap();

        let (match_spec, _) = package_spec.to_match_spec(package_name).unwrap();

        // Verify the build constraint is preserved
        assert_eq!(
            match_spec.name,
            Some(PackageNameMatcher::Exact(
                PackageName::try_from("tk").unwrap()
            ))
        );
        assert_eq!(match_spec.version, Some(VersionSpec::Any));
        assert_eq!(match_spec.build, Some(build_matcher));
    }

    #[test]
    fn test_to_match_spec_preserves_build_constraint_with_specific_version() {
        // Test case: dependency with specific version and build constraint
        // e.g., tk = { version = "8.6.13", build = "xft*" }
        let version = VersionSpec::from_str("8.6.13", ParseStrictness::Lenient).unwrap();
        let build_matcher: StringMatcher = "xft*".parse().unwrap();
        let binary_spec = pbt::BinaryPackageSpec {
            version: Some(version.clone()),
            build: Some(build_matcher.clone()),
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            md5: None,
            sha256: None,
            url: None,
            license: None,
        };

        let package_spec = pbt::PackageSpec::Binary(binary_spec);
        let package_name = PackageName::try_from("tk").unwrap();

        let (match_spec, _) = package_spec.to_match_spec(package_name).unwrap();

        // Verify both version and build constraint are preserved
        assert_eq!(
            match_spec.name,
            Some(PackageNameMatcher::Exact(
                PackageName::try_from("tk").unwrap()
            ))
        );
        assert_eq!(match_spec.version, Some(version));
        assert_eq!(match_spec.build, Some(build_matcher));
    }

    #[test]
    fn test_to_match_spec_without_build_constraint() {
        // Test case: dependency with wildcard version but no build constraint
        // e.g., python = "*"
        let binary_spec = pbt::BinaryPackageSpec {
            version: Some(VersionSpec::Any),
            build: None,
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            md5: None,
            sha256: None,
            url: None,
            license: None,
        };

        let package_spec = pbt::PackageSpec::Binary(binary_spec);
        let package_name = PackageName::try_from("python").unwrap();

        let (match_spec, _) = package_spec.to_match_spec(package_name).unwrap();

        // Verify the match spec is correct
        assert_eq!(
            match_spec.name,
            Some(PackageNameMatcher::Exact(
                PackageName::try_from("python").unwrap()
            ))
        );
        assert_eq!(match_spec.version, Some(VersionSpec::Any));
        assert_eq!(match_spec.build, None);
    }
}
