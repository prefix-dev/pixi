//! PyPI to conda package name mapping.
//!
//! This module provides functionality to map PyPI package names to their
//! corresponding conda-forge package names using the parselmouth mapping service.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime},
};

use indexmap::IndexMap;

use miette::Diagnostic;
use rattler_conda_types::{
    ChannelUrl, MatchSpec, PackageName, ParseStrictness, Platform, VersionSpec,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Base URL for the PyPI to conda mapping API (without channel suffix).
const MAPPING_BASE_URL: &str = "https://conda-mapping.prefix.dev/pypi-to-conda-v1";

/// Base subdirectory within the cache for storing mapping files.
const CACHE_SUBDIR: &str = "pypi-conda-mapping";

/// Cache validity duration (24 hours).
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Errors that can occur during PyPI to conda mapping.
#[derive(Debug, Error, Diagnostic)]
pub enum MappingError {
    /// Failed to fetch mapping from the API.
    #[error("failed to fetch conda mapping for '{0}'")]
    FetchError(String, #[source] reqwest::Error),

    /// Failed to parse the mapping response.
    #[error("failed to parse mapping response for '{0}'")]
    ParseError(String, #[source] serde_json::Error),

    /// Package not found in the mapping.
    #[error("PyPI package '{0}' has no conda mapping")]
    PackageNotFound(String),

    /// Invalid version specifier conversion.
    #[error("failed to convert version specifier '{0}' to conda format: {1}")]
    VersionConversionError(String, String),

    /// Invalid package name.
    #[error("invalid conda package name '{0}'")]
    InvalidPackageName(
        String,
        #[source] rattler_conda_types::InvalidPackageNameError,
    ),
}

/// Response format from the PyPI to conda mapping API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PyPiPackageLookup {
    /// Format version of the response.
    pub format_version: String,

    /// Channel (e.g., "conda-forge").
    pub channel: String,

    /// The PyPI package name.
    pub pypi_name: String,

    /// Mapping of PyPI versions to the best-matching conda package name.
    /// Key is PyPI version string, value is the conda package name (selected server-side
    /// using Levenshtein distance to the PyPI name).
    /// Uses IndexMap to preserve insertion order from the API (latest version is last).
    pub conda_versions: IndexMap<String, String>,
}

/// A successfully mapped conda dependency.
#[derive(Debug, Clone)]
pub struct MappedCondaDependency {
    /// The conda package name.
    pub name: PackageName,

    /// Optional version specification.
    pub version_spec: Option<VersionSpec>,
}

impl MappedCondaDependency {
    /// Convert to a conda MatchSpec.
    pub fn to_match_spec(&self) -> MatchSpec {
        MatchSpec {
            name: Some(rattler_conda_types::PackageNameMatcher::Exact(
                self.name.clone(),
            )),
            version: self.version_spec.clone(),
            ..Default::default()
        }
    }
}

/// Mapper for converting PyPI packages to conda packages.
pub struct PyPiToCondaMapper {
    cache_dir: Option<PathBuf>,
    client: reqwest::Client,
    /// The channel name to use for mapping (e.g., "conda-forge").
    channel_name: String,
    /// Inline mappings for testing (bypasses cache and API).
    #[cfg(test)]
    inline_mappings: Option<IndexMap<String, PyPiPackageLookup>>,
}

impl PyPiToCondaMapper {
    /// Create a new mapper with the given cache directory and channel name.
    pub fn new(cache_dir: Option<PathBuf>, channel_name: String) -> Self {
        Self {
            cache_dir,
            client: reqwest::Client::new(),
            channel_name,
            #[cfg(test)]
            inline_mappings: None,
        }
    }

    /// Create a mapper with inline mappings for testing.
    /// This bypasses the cache and API, using only the provided mappings.
    #[cfg(test)]
    pub fn with_inline_mappings(mappings: IndexMap<String, PyPiPackageLookup>) -> Self {
        Self {
            cache_dir: None,
            client: reqwest::Client::new(),
            channel_name: "test".to_string(),
            inline_mappings: Some(mappings),
        }
    }

    /// Get the cache file path for a normalized package name.
    fn cache_path(&self, normalized_name: &str) -> Option<PathBuf> {
        self.cache_dir.as_ref().map(|dir| {
            dir.join(CACHE_SUBDIR)
                .join(&self.channel_name)
                .join(format!("{}.json", normalized_name))
        })
    }

    /// Check if a cached file is still valid.
    fn is_cache_valid(path: &Path) -> bool {
        if let Ok(metadata) = std::fs::metadata(path)
            && let Ok(modified) = metadata.modified()
            && let Ok(elapsed) = SystemTime::now().duration_since(modified)
        {
            return elapsed < CACHE_TTL;
        }

        false
    }

    /// Read a mapping from the cache.
    fn read_from_cache(&self, normalized_name: &str) -> Option<PyPiPackageLookup> {
        let cache_path = self.cache_path(normalized_name)?;

        if !cache_path.exists() || !Self::is_cache_valid(&cache_path) {
            return None;
        }

        let content = std::fs::read_to_string(&cache_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Write a mapping to the cache.
    fn write_to_cache(&self, normalized_name: &str, lookup: &PyPiPackageLookup) {
        let Some(cache_path) = self.cache_path(normalized_name) else {
            return;
        };

        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Ok(content) = serde_json::to_string(lookup) {
            let _ = std::fs::write(cache_path, content);
        }
    }

    /// Fetch a mapping from the API.
    async fn fetch_from_api(&self, pypi_name: &str) -> Result<PyPiPackageLookup, MappingError> {
        let url = format!(
            "{}/{}/{}.json",
            MAPPING_BASE_URL, self.channel_name, pypi_name
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| MappingError::FetchError(pypi_name.to_string(), e))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(MappingError::PackageNotFound(pypi_name.to_string()));
        }

        let text = response
            .text()
            .await
            .map_err(|e| MappingError::FetchError(pypi_name.to_string(), e))?;

        let lookup: PyPiPackageLookup = serde_json::from_str(&text)
            .map_err(|e| MappingError::ParseError(pypi_name.to_string(), e))?;

        Ok(lookup)
    }

    /// Get the mapping for a PyPI package, using cache if available.
    pub async fn get_mapping(&self, pypi_name: &str) -> Result<PyPiPackageLookup, MappingError> {
        // Check inline mappings first (test-only)
        #[cfg(test)]
        if let Some(ref mappings) = self.inline_mappings {
            return mappings
                .get(pypi_name)
                .cloned()
                .ok_or_else(|| MappingError::PackageNotFound(pypi_name.to_string()));
        }

        // Try cache
        if let Some(cached) = self.read_from_cache(pypi_name) {
            return Ok(cached);
        }

        // Fetch from API
        let lookup = self.fetch_from_api(pypi_name).await?;

        // Write to cache
        self.write_to_cache(pypi_name, &lookup);

        Ok(lookup)
    }

    /// Extract the conda package name from a lookup.
    ///
    /// Returns the best-matching conda package name for the latest version.
    fn extract_conda_name(lookup: &PyPiPackageLookup) -> Option<String> {
        // The last entry is the latest version.
        lookup.conda_versions.values().last().cloned()
    }

    /// Convert PEP 440 version specifiers to conda VersionSpec.
    ///
    /// This handles common specifiers directly and transforms PEP 440-specific
    /// syntax like `===` (arbitrary equality) to conda equivalents.
    fn convert_version_specifiers(
        specifiers: &pep508_rs::VersionOrUrl<pep508_rs::VerbatimUrl>,
    ) -> Result<Option<VersionSpec>, MappingError> {
        let pep508_rs::VersionOrUrl::VersionSpecifier(specs) = specifiers else {
            // URL-based dependency, no version constraint
            return Ok(None);
        };

        if specs.is_empty() {
            return Ok(None);
        }

        // Handle PEP 440-specific operators that conda doesn't understand
        let spec_str = specs.to_string();
        let converted = Self::convert_pep440_operators(&spec_str);

        VersionSpec::from_str(&converted, ParseStrictness::Lenient)
            .map(Some)
            .map_err(|e| MappingError::VersionConversionError(spec_str, e.to_string()))
    }

    /// Convert PEP 440-specific operators to conda-compatible equivalents.
    fn convert_pep440_operators(spec_str: &str) -> String {
        // Handle === (arbitrary equality): ===1.0.0 becomes ==1.0.0
        spec_str.replace("===", "==")
    }

    /// Create a marker environment for the given platform and Python version.
    ///
    /// This converts a rattler Platform to a pep508_rs MarkerEnvironment that can be used
    /// to evaluate PEP 508 environment markers.
    fn create_marker_environment(platform: Platform) -> pep508_rs::MarkerEnvironment {
        // Map Platform to Python's sys.platform and other marker values
        let (sys_platform, os_name, platform_system, platform_machine) = match platform {
            Platform::Linux64 => ("linux", "posix", "Linux", "x86_64"),
            Platform::LinuxAarch64 => ("linux", "posix", "Linux", "aarch64"),
            Platform::LinuxPpc64le => ("linux", "posix", "Linux", "ppc64le"),
            Platform::LinuxS390X => ("linux", "posix", "Linux", "s390x"),
            Platform::LinuxArmV6l => ("linux", "posix", "Linux", "armv6l"),
            Platform::LinuxArmV7l => ("linux", "posix", "Linux", "armv7l"),
            Platform::Linux32 => ("linux", "posix", "Linux", "i686"),
            Platform::Osx64 => ("darwin", "posix", "Darwin", "x86_64"),
            Platform::OsxArm64 => ("darwin", "posix", "Darwin", "arm64"),
            Platform::Win64 => ("win32", "nt", "Windows", "AMD64"),
            Platform::Win32 => ("win32", "nt", "Windows", "x86"),
            Platform::WinArm64 => ("win32", "nt", "Windows", "ARM64"),
            Platform::NoArch => ("linux", "posix", "Linux", "x86_64"),
            _ => ("linux", "posix", "Linux", "x86_64"), // Default to linux x86_64 for unknown platforms
        };

        // Use builder pattern to create MarkerEnvironment
        // Note: Python version fields are set to dummy values since we strip non-system fields
        // from markers before evaluation. Only system fields (os_name, platform_*, sys_platform) matter.
        pep508_rs::MarkerEnvironment::try_from(pep508_rs::MarkerEnvironmentBuilder {
            implementation_name: "cpython",
            implementation_version: "1.0.0",
            os_name,
            platform_machine,
            platform_python_implementation: "CPython",
            platform_release: "",
            platform_system,
            platform_version: "",
            python_full_version: "1.0.0",
            python_version: "1.0.0",
            sys_platform,
        })
        .expect("Failed to create MarkerEnvironment")
    }

    /// Check if a marker contains any non-system fields.
    ///
    /// Non-system fields include python_version, python_full_version, implementation_*,
    /// platform_python_implementation, platform_release, platform_version, and extra.
    fn contains_non_system_fields(marker: &pep508_rs::MarkerTree) -> bool {
        // Check for both snake_case and PascalCase variants to handle different
        // pep508_rs versions and Debug format variations
        // Fields can be found here: https://peps.python.org/pep-0508/#environment-markers
        let non_system_fields = [
            "python_version",
            "PythonVersion",
            "python_full_version",
            "PythonFullVersion",
            "implementation_name",
            "ImplementationName",
            "implementation_version",
            "ImplementationVersion",
            "platform_python_implementation",
            "PlatformPythonImplementation",
            "platform_release",
            "PlatformRelease",
            "platform_version",
            "PlatformVersion",
            "extra",
            "Extra",
        ];

        let marker_str = format!("{:?}", marker);
        non_system_fields.iter().any(|f| marker_str.contains(f))
    }

    /// Check if a requirement should be skipped for the given platform.
    ///
    /// Returns true if the requirement should be skipped (excluded), false if it should be included.
    ///
    /// For NoArch platforms, ALL dependencies with markers (system or non-system) are excluded
    /// because noarch packages must be platform-independent.
    ///
    /// If a marker contains ANY non-system fields (python_version, python_full_version,
    /// implementation details, extras, etc.), the dependency is excluded entirely.
    /// This conservative approach prevents incorrectly including dependencies with
    /// version constraints we cannot evaluate at recipe generation time.
    fn should_skip_requirement(
        req: &pep508_rs::Requirement<pep508_rs::VerbatimUrl>,
        platform: Platform,
    ) -> bool {
        // If there are no markers, always include (don't skip)
        if req.marker == pep508_rs::MarkerTree::default() {
            return false;
        }

        // For NoArch platform, exclude ALL dependencies with markers
        // NoArch packages must be platform-independent
        if platform == Platform::NoArch {
            return true;
        }

        // If the marker contains any non-system fields, exclude it entirely
        // This is conservative: better to exclude and let users add manually
        // than to include incorrectly and cause build failures
        if Self::contains_non_system_fields(&req.marker) {
            tracing::debug!(
                "Excluding dependency '{}' because marker {:?} contains non-system fields (python_version, etc.)",
                req.name,
                req.marker
            );
            return true;
        }

        // At this point, marker contains only system fields
        // Evaluate against the platform
        let marker_env = Self::create_marker_environment(platform);
        let result = req.marker.evaluate(&marker_env, &[]);

        tracing::debug!(
            "Dependency '{}' with system-only marker {:?} evaluates to {} for platform {}",
            req.name,
            req.marker,
            result,
            platform
        );

        // Skip if marker evaluates to false (requirement not applicable to this platform)
        !result
    }

    /// Map a list of PEP 508 requirements to conda MatchSpecs.
    ///
    /// Returns a list of successfully mapped dependencies. Unmapped packages
    /// are logged as warnings and skipped.
    pub async fn map_requirements(
        &self,
        requirements: &[pep508_rs::Requirement<pep508_rs::VerbatimUrl>],
        platform: Platform,
    ) -> Result<Vec<MappedCondaDependency>, MappingError> {
        let mut mapped = Vec::new();

        for req in requirements {
            // Evaluate markers against the target platform
            if Self::should_skip_requirement(req, platform) {
                tracing::debug!(
                    "Skipping dependency '{}' due to environment marker evaluation: {:?}",
                    req.name,
                    req.marker
                );
                continue;
            }

            // Get the mapping
            let lookup = match self.get_mapping(req.name.as_ref()).await {
                Ok(l) => l,
                Err(MappingError::PackageNotFound(_)) => {
                    tracing::warn!(
                        "PyPI package '{}' has no conda-forge mapping, skipping",
                        req.name
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };

            // Extract the conda package name
            let conda_name_str = match Self::extract_conda_name(&lookup) {
                Some(n) => n,
                None => {
                    tracing::warn!(
                        "No conda package names found in mapping for '{}', skipping",
                        req.name
                    );
                    continue;
                }
            };

            // Parse conda package name
            let conda_name = PackageName::from_str(&conda_name_str)
                .map_err(|e| MappingError::InvalidPackageName(conda_name_str.clone(), e))?;

            // Convert version specifiers
            let version_spec = if let Some(ref version_or_url) = req.version_or_url {
                match Self::convert_version_specifiers(version_or_url) {
                    Ok(spec) => spec,
                    Err(e) => {
                        tracing::warn!(
                            "Failed to convert version specifier for '{}': {}, using unconstrained version",
                            req.name,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            };

            mapped.push(MappedCondaDependency {
                name: conda_name,
                version_spec,
            });
        }

        Ok(mapped)
    }
}

/// Filter mapped PyPI dependencies, returning only those not already specified
/// in Pixi's run dependencies.
///
/// This implements the merging behavior where Pixi dependencies take precedence
/// over inferred pyproject.toml dependencies. Dependencies not specified in
/// `skip_packages` are returned as MatchSpecs ready to be added to requirements.
pub fn filter_mapped_pypi_deps(
    mapped_deps: &[MappedCondaDependency],
    skip_packages: &HashSet<pixi_build_types::SourcePackageName>,
) -> Vec<MatchSpec> {
    mapped_deps
        .iter()
        .filter(|dep| {
            let pkg_name = pixi_build_types::SourcePackageName::from(dep.name.as_normalized());
            !skip_packages.contains(&pkg_name)
        })
        .map(|dep| dep.to_match_spec())
        .collect()
}

/// Extract the channel name from a channel URL.
///
/// Returns the last path segment (e.g., "conda-forge" from
/// "https://prefix.dev/conda-forge").
pub fn extract_channel_name(channel: &ChannelUrl) -> Option<&str> {
    channel.as_str().trim_end_matches('/').rsplit('/').next()
}

/// Map PyPI requirements to conda dependencies using the first channel that provides a valid mapping.
///
/// Tries each channel in order and returns the mapped dependencies from the first
/// channel that successfully maps at least one dependency. Returns an empty Vec
/// if no channel provides a mapping.
///
/// The `context` parameter is used for logging (e.g., "project dependencies" or
/// "build-system requirements").
pub async fn map_requirements_with_channels(
    requirements: &[pep508_rs::Requirement<pep508_rs::VerbatimUrl>],
    channels: &[ChannelUrl],
    cache_dir: &Option<PathBuf>,
    context: &str,
    platform: Platform,
) -> Vec<MappedCondaDependency> {
    for channel in channels {
        if let Some(channel_name) = extract_channel_name(channel) {
            let mapper = PyPiToCondaMapper::new(cache_dir.clone(), channel_name.to_string());
            match mapper.map_requirements(requirements, platform).await {
                Ok(deps) if !deps.is_empty() => {
                    tracing::debug!(
                        "Using PyPI-to-conda mapping for {} from channel '{}'",
                        context,
                        channel_name
                    );
                    return deps;
                }
                Ok(_) => {
                    tracing::warn!(
                        "No PyPI-to-conda mapping found for {} in channel '{}'",
                        context,
                        channel_name
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to get PyPI-to-conda mapping for {} in channel '{}': {}",
                        context,
                        channel_name,
                        e
                    );
                }
            }
        }
    }
    Vec::new()
}

/// Build tools that require specific compilers.
///
/// Maps PyPI package names to the compilers they require. This is used to
/// automatically detect compilers from `build-system.requires` in pyproject.toml.
const BUILD_TOOL_COMPILER_MAPPINGS: &[(&str, &[&str])] =
    &[("maturin", &["rust"]), ("setuptools-rust", &["rust"])];

/// Detect compilers required by build tools in `build-system.requires`.
///
/// Examines the list of PEP 508 requirements and returns any compilers that
/// should be automatically added based on the detected build tools.
pub fn detect_compilers_from_build_requirements(
    requirements: &[pep508_rs::Requirement<pep508_rs::VerbatimUrl>],
) -> Vec<String> {
    let mut detected_compilers = HashSet::new();

    for req in requirements {
        let package_name = req.name.as_ref();

        for (tool_name, compilers) in BUILD_TOOL_COMPILER_MAPPINGS {
            if package_name == *tool_name {
                detected_compilers.extend(compilers.iter().map(|s| s.to_string()));
            }
        }
    }

    detected_compilers.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_pep440_operators() {
        assert_eq!(
            PyPiToCondaMapper::convert_pep440_operators(">=1.0,<2.0"),
            ">=1.0,<2.0"
        );
        assert_eq!(
            PyPiToCondaMapper::convert_pep440_operators("===1.0.0"),
            "==1.0.0"
        );
        assert_eq!(
            PyPiToCondaMapper::convert_pep440_operators("~=1.4.2"),
            "~=1.4.2"
        );
    }

    #[test]
    fn test_extract_conda_name() {
        let lookup = PyPiPackageLookup {
            format_version: "1".to_string(),
            channel: "conda-forge".to_string(),
            pypi_name: "requests".to_string(),
            conda_versions: IndexMap::from([
                ("2.31.0".to_string(), "requests".to_string()),
                ("2.32.0".to_string(), "requests".to_string()),
            ]),
        };

        assert_eq!(
            PyPiToCondaMapper::extract_conda_name(&lookup),
            Some("requests".to_string())
        );
    }

    #[test]
    fn test_extract_conda_name_empty() {
        let lookup = PyPiPackageLookup {
            format_version: "1".to_string(),
            channel: "conda-forge".to_string(),
            pypi_name: "unknown".to_string(),
            conda_versions: IndexMap::new(),
        };

        assert_eq!(PyPiToCondaMapper::extract_conda_name(&lookup), None);
    }

    #[test]
    fn test_extract_conda_name_returns_value() {
        // The API returns the best-matching conda package directly
        let lookup = PyPiPackageLookup {
            format_version: "1.0".to_string(),
            channel: "conda-forge".to_string(),
            pypi_name: "jinja2".to_string(),
            conda_versions: IndexMap::from([("3.1.3".to_string(), "jinja2".to_string())]),
        };

        assert_eq!(
            PyPiToCondaMapper::extract_conda_name(&lookup),
            Some("jinja2".to_string())
        );
    }

    #[tokio::test]
    async fn test_map_requirements_with_inline_mappings() {
        let mappings = IndexMap::from([
            (
                "requests".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "requests".to_string(),
                    conda_versions: IndexMap::from([(
                        "2.31.0".to_string(),
                        "requests".to_string(),
                    )]),
                },
            ),
            (
                "flask".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "flask".to_string(),
                    conda_versions: IndexMap::from([("2.0.0".to_string(), "flask".to_string())]),
                },
            ),
        ]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        let requirements = vec![
            pep508_rs::Requirement::from_str("requests>=2.0").unwrap(),
            pep508_rs::Requirement::from_str("flask").unwrap(),
        ];

        let mapped = mapper
            .map_requirements(&requirements, Platform::Linux64)
            .await
            .unwrap();

        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].name.as_normalized(), "requests");
        assert_eq!(
            mapped[0].version_spec.as_ref().unwrap().to_string(),
            ">=2.0"
        );
        assert_eq!(mapped[1].name.as_normalized(), "flask");
        assert!(mapped[1].version_spec.is_none());
    }

    fn make_mapped_dep(name: &str, version_spec: Option<&str>) -> MappedCondaDependency {
        MappedCondaDependency {
            name: PackageName::from_str(name).unwrap(),
            version_spec: version_spec
                .map(|s| VersionSpec::from_str(s, ParseStrictness::Lenient).unwrap()),
        }
    }

    #[test]
    fn test_filter_mapped_pypi_deps_without_pixi_deps() {
        // When no Pixi deps are specified, all mapped deps should pass through
        let mapped_deps = vec![
            make_mapped_dep("requests", Some(">=2.0")),
            make_mapped_dep("flask", None),
        ];

        let skip_packages: HashSet<pixi_build_types::SourcePackageName> = HashSet::new();

        let result = filter_mapped_pypi_deps(&mapped_deps, &skip_packages);

        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|r| r.to_string().contains("requests")));
        assert!(result.iter().any(|r| r.to_string().contains("flask")));
    }

    #[test]
    fn test_filter_mapped_pypi_deps_override_but_others_preserved() {
        // When Pixi specifies some deps, those should be filtered out
        // but other deps should still pass through
        let mapped_deps = vec![
            make_mapped_dep("requests", Some(">=2.0")),
            make_mapped_dep("flask", Some(">=1.0")),
            make_mapped_dep("numpy", None),
        ];

        // Pixi specifies "requests" - it should be filtered out
        let skip_packages: HashSet<pixi_build_types::SourcePackageName> =
            HashSet::from([pixi_build_types::SourcePackageName::from("requests")]);

        let result = filter_mapped_pypi_deps(&mapped_deps, &skip_packages);

        // requests should NOT be in result (filtered by Pixi override)
        // flask and numpy should be in result
        assert_eq!(result.len(), 2);
        assert!(!result.iter().any(|r| r.to_string().contains("requests")));
        assert!(result.iter().any(|r| r.to_string().contains("flask")));
        assert!(result.iter().any(|r| r.to_string().contains("numpy")));
    }

    #[test]
    fn test_filter_mapped_pypi_deps_all_filtered_when_all_in_pixi() {
        // When all mapped deps are already in Pixi, nothing should pass through
        let mapped_deps = vec![
            make_mapped_dep("requests", Some(">=2.0")),
            make_mapped_dep("flask", None),
        ];

        let skip_packages: HashSet<pixi_build_types::SourcePackageName> = HashSet::from([
            pixi_build_types::SourcePackageName::from("requests"),
            pixi_build_types::SourcePackageName::from("flask"),
        ]);

        let result = filter_mapped_pypi_deps(&mapped_deps, &skip_packages);

        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_channel_name() {
        use url::Url;

        // Test extracting channel name from various URL formats
        let url1 = ChannelUrl::from(Url::parse("https://prefix.dev/conda-forge").unwrap());
        assert_eq!(extract_channel_name(&url1), Some("conda-forge"));

        let url2 = ChannelUrl::from(Url::parse("https://conda.anaconda.org/conda-forge/").unwrap());
        assert_eq!(extract_channel_name(&url2), Some("conda-forge"));

        let url3 = ChannelUrl::from(Url::parse("https://example.com/my-channel").unwrap());
        assert_eq!(extract_channel_name(&url3), Some("my-channel"));
    }

    #[tokio::test]
    async fn test_marker_evaluation_linux() {
        let mappings = IndexMap::from([(
            "typing-extensions".to_string(),
            PyPiPackageLookup {
                format_version: "1".to_string(),
                channel: "conda-forge".to_string(),
                pypi_name: "typing-extensions".to_string(),
                conda_versions: IndexMap::from([(
                    "4.0.0".to_string(),
                    "typing-extensions".to_string(),
                )]),
            },
        )]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        // Requirement with sys_platform == "linux" marker - should be included on Linux
        let requirements = vec![
            pep508_rs::Requirement::from_str("typing-extensions; sys_platform == 'linux'").unwrap(),
        ];

        let mapped_linux = mapper
            .map_requirements(&requirements, Platform::Linux64)
            .await
            .unwrap();
        assert_eq!(mapped_linux.len(), 1, "Should include on Linux64");

        let mapped_win = mapper
            .map_requirements(&requirements, Platform::Win64)
            .await
            .unwrap();
        assert_eq!(mapped_win.len(), 0, "Should exclude on Win64");
    }

    #[tokio::test]
    async fn test_marker_evaluation_windows() {
        let mappings = IndexMap::from([(
            "colorama".to_string(),
            PyPiPackageLookup {
                format_version: "1".to_string(),
                channel: "conda-forge".to_string(),
                pypi_name: "colorama".to_string(),
                conda_versions: IndexMap::from([("0.4.6".to_string(), "colorama".to_string())]),
            },
        )]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        // Requirement with sys_platform == "win32" marker - should be included on Windows
        let requirements =
            vec![pep508_rs::Requirement::from_str("colorama; sys_platform == 'win32'").unwrap()];

        let mapped_win = mapper
            .map_requirements(&requirements, Platform::Win64)
            .await
            .unwrap();
        assert_eq!(mapped_win.len(), 1, "Should include on Win64");

        let mapped_linux = mapper
            .map_requirements(&requirements, Platform::Linux64)
            .await
            .unwrap();
        assert_eq!(mapped_linux.len(), 0, "Should exclude on Linux64");
    }

    #[tokio::test]
    async fn test_marker_evaluation_darwin() {
        let mappings = IndexMap::from([(
            "pyobjc-core".to_string(),
            PyPiPackageLookup {
                format_version: "1".to_string(),
                channel: "conda-forge".to_string(),
                pypi_name: "pyobjc-core".to_string(),
                conda_versions: IndexMap::from([("9.0".to_string(), "pyobjc-core".to_string())]),
            },
        )]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        // Requirement with sys_platform == "darwin" marker - should be included on macOS
        let requirements = vec![
            pep508_rs::Requirement::from_str("pyobjc-core; sys_platform == 'darwin'").unwrap(),
        ];

        let mapped_osx = mapper
            .map_requirements(&requirements, Platform::Osx64)
            .await
            .unwrap();
        assert_eq!(mapped_osx.len(), 1, "Should include on Osx64");

        let mapped_linux = mapper
            .map_requirements(&requirements, Platform::Linux64)
            .await
            .unwrap();
        assert_eq!(mapped_linux.len(), 0, "Should exclude on Linux64");
    }

    #[tokio::test]
    async fn test_marker_evaluation_no_marker() {
        let mapper = PyPiToCondaMapper::with_inline_mappings(IndexMap::from([(
            "requests".to_string(),
            PyPiPackageLookup {
                format_version: "1".to_string(),
                channel: "conda-forge".to_string(),
                pypi_name: "requests".to_string(),
                conda_versions: IndexMap::from([("2.31.0".to_string(), "requests".to_string())]),
            },
        )]));

        let requirements = vec![pep508_rs::Requirement::from_str("requests").unwrap()];

        for &platform in &[Platform::Linux64, Platform::Win64, Platform::Osx64] {
            let mapped = mapper
                .map_requirements(&requirements, platform)
                .await
                .unwrap();
            assert_eq!(mapped.len(), 1, "Should include on {:?}", platform);
        }
    }

    #[tokio::test]
    async fn test_marker_python_full_version_excluded() {
        let mappings = IndexMap::from([(
            "importlib-metadata".to_string(),
            PyPiPackageLookup {
                format_version: "1".to_string(),
                channel: "conda-forge".to_string(),
                pypi_name: "importlib-metadata".to_string(),
                conda_versions: IndexMap::from([(
                    "6.0.0".to_string(),
                    "importlib-metadata".to_string(),
                )]),
            },
        )]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        // Requirement with python_full_version marker
        let requirements = vec![
            pep508_rs::Requirement::from_str("importlib-metadata; python_full_version < '3.10.0'")
                .unwrap(),
        ];

        let mapped = mapper
            .map_requirements(&requirements, Platform::Linux64)
            .await
            .unwrap();
        assert_eq!(
            mapped.len(),
            0,
            "Should exclude dependency with python_full_version marker (non-system field)"
        );
    }

    #[tokio::test]
    async fn test_marker_combined_python_and_system() {
        let mappings = IndexMap::from([(
            "pywin32".to_string(),
            PyPiPackageLookup {
                format_version: "1".to_string(),
                channel: "conda-forge".to_string(),
                pypi_name: "pywin32".to_string(),
                conda_versions: IndexMap::from([("306".to_string(), "pywin32".to_string())]),
            },
        )]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        // Requirement with both Python version AND system marker
        // The python_version part is stripped, leaving only sys_platform == 'win32' to evaluate
        let requirements = vec![
            pep508_rs::Requirement::from_str(
                "pywin32; sys_platform == 'win32' and python_version >= '3.8'",
            )
            .unwrap(),
        ];

        let mapped_linux = mapper
            .map_requirements(&requirements, Platform::Linux64)
            .await
            .unwrap();
        assert_eq!(
            mapped_linux.len(),
            0,
            "Should exclude on Linux because sys_platform == 'win32' evaluates to false (after stripping python_version)"
        );

        let mapped_win = mapper
            .map_requirements(&requirements, Platform::Win64)
            .await
            .unwrap();
        assert_eq!(
            mapped_win.len(),
            0,
            "Should exclude because we can't check python_version, even though sys_platform == 'win32' is true"
        );
    }

    #[tokio::test]
    async fn test_all_system_marker_styles() {
        // Test all supported system marker types in a compact table-driven test
        let mappings = IndexMap::from([(
            "test-pkg".to_string(),
            PyPiPackageLookup {
                format_version: "1".to_string(),
                channel: "conda-forge".to_string(),
                pypi_name: "test-pkg".to_string(),
                conda_versions: IndexMap::from([("1.0.0".to_string(), "test-pkg".to_string())]),
            },
        )]);

        let test_cases = vec![
            // (marker_expression, platform, should_include)
            // sys_platform markers
            ("sys_platform == 'linux'", Platform::Linux64, true),
            ("sys_platform == 'linux'", Platform::LinuxAarch64, true),
            ("sys_platform == 'linux'", Platform::Win64, false),
            ("sys_platform == 'linux'", Platform::Osx64, false),
            ("sys_platform == 'win32'", Platform::Win64, true),
            ("sys_platform == 'win32'", Platform::Win32, true),
            ("sys_platform == 'win32'", Platform::WinArm64, true),
            ("sys_platform == 'win32'", Platform::Linux64, false),
            ("sys_platform == 'darwin'", Platform::Osx64, true),
            ("sys_platform == 'darwin'", Platform::OsxArm64, true),
            ("sys_platform == 'darwin'", Platform::Linux64, false),
            // platform_system markers
            ("platform_system == 'Linux'", Platform::Linux64, true),
            ("platform_system == 'Linux'", Platform::LinuxAarch64, true),
            ("platform_system == 'Linux'", Platform::Win64, false),
            ("platform_system == 'Windows'", Platform::Win64, true),
            ("platform_system == 'Windows'", Platform::Win32, true),
            ("platform_system == 'Windows'", Platform::Linux64, false),
            ("platform_system == 'Darwin'", Platform::Osx64, true),
            ("platform_system == 'Darwin'", Platform::OsxArm64, true),
            ("platform_system == 'Darwin'", Platform::Linux64, false),
            // os_name markers
            ("os_name == 'posix'", Platform::Linux64, true),
            ("os_name == 'posix'", Platform::Osx64, true),
            ("os_name == 'posix'", Platform::Win64, false),
            ("os_name == 'nt'", Platform::Win64, true),
            ("os_name == 'nt'", Platform::Linux64, false),
            // platform_machine markers
            ("platform_machine == 'x86_64'", Platform::Linux64, true),
            ("platform_machine == 'x86_64'", Platform::Osx64, true),
            (
                "platform_machine == 'x86_64'",
                Platform::LinuxAarch64,
                false,
            ),
            (
                "platform_machine == 'aarch64'",
                Platform::LinuxAarch64,
                true,
            ),
            ("platform_machine == 'aarch64'", Platform::Linux64, false),
            ("platform_machine == 'arm64'", Platform::OsxArm64, true),
            ("platform_machine == 'arm64'", Platform::Osx64, false),
            ("platform_machine == 'AMD64'", Platform::Win64, true),
            ("platform_machine == 'AMD64'", Platform::Win32, false),
        ];

        for (marker, platform, should_include) in test_cases {
            let mapper = PyPiToCondaMapper::with_inline_mappings(mappings.clone());
            let requirement_str = format!("test-pkg; {}", marker);
            let requirements = vec![pep508_rs::Requirement::from_str(&requirement_str).unwrap()];

            let mapped = mapper
                .map_requirements(&requirements, platform)
                .await
                .unwrap();

            let expected_len = if should_include { 1 } else { 0 };
            assert_eq!(
                mapped.len(),
                expected_len,
                "Marker '{}' on {:?} should {} the package",
                marker,
                platform,
                if should_include { "include" } else { "exclude" }
            );
        }
    }
    #[test]
    fn test_detect_compilers_maturin() {
        let requirements = vec![pep508_rs::Requirement::from_str("maturin>=1.0,<2.0").unwrap()];

        let compilers = detect_compilers_from_build_requirements(&requirements);

        assert_eq!(compilers, vec!["rust"]);
    }

    #[test]
    fn test_detect_compilers_setuptools_rust() {
        let requirements = vec![pep508_rs::Requirement::from_str("setuptools-rust>=1.0").unwrap()];

        let compilers = detect_compilers_from_build_requirements(&requirements);

        assert_eq!(compilers, vec!["rust"]);
    }

    #[test]
    fn test_detect_compilers_no_special_tools() {
        let requirements = vec![
            pep508_rs::Requirement::from_str("setuptools>=42").unwrap(),
            pep508_rs::Requirement::from_str("wheel").unwrap(),
        ];

        let compilers = detect_compilers_from_build_requirements(&requirements);

        assert!(compilers.is_empty());
    }

    #[test]
    fn test_detect_compilers_deduplicates() {
        // Both maturin and setuptools-rust require rust - should only appear once
        let requirements = vec![
            pep508_rs::Requirement::from_str("maturin>=1.0").unwrap(),
            pep508_rs::Requirement::from_str("setuptools-rust>=1.0").unwrap(),
        ];

        let compilers = detect_compilers_from_build_requirements(&requirements);

        assert_eq!(compilers, vec!["rust"]);
    }

    #[tokio::test]
    async fn test_noarch_excludes_all_marker_dependencies() {
        let mappings = IndexMap::from([
            (
                "requests".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "requests".to_string(),
                    conda_versions: IndexMap::from([(
                        "2.31.0".to_string(),
                        "requests".to_string(),
                    )]),
                },
            ),
            (
                "colorama".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "colorama".to_string(),
                    conda_versions: IndexMap::from([("0.4.6".to_string(), "colorama".to_string())]),
                },
            ),
            (
                "importlib-metadata".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "importlib-metadata".to_string(),
                    conda_versions: IndexMap::from([(
                        "6.0.0".to_string(),
                        "importlib-metadata".to_string(),
                    )]),
                },
            ),
        ]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        // Test with various marker types - all should be excluded for NoArch
        let requirements = vec![
            // No marker - should be included
            pep508_rs::Requirement::from_str("requests").unwrap(),
            // System marker - should be excluded for NoArch
            pep508_rs::Requirement::from_str("colorama; sys_platform == 'win32'").unwrap(),
            // Non-system marker - should be excluded for NoArch
            pep508_rs::Requirement::from_str("importlib-metadata; python_full_version < '3.10.0'")
                .unwrap(),
        ];

        let mapped = mapper
            .map_requirements(&requirements, Platform::NoArch)
            .await
            .unwrap();

        // Only the dependency without markers should be included
        assert_eq!(
            mapped.len(),
            1,
            "NoArch should only include dependencies without markers"
        );
        assert_eq!(
            mapped[0].name.as_normalized(),
            "requests",
            "NoArch should include the unmarked dependency"
        );
    }

    #[tokio::test]
    async fn test_noarch_excludes_system_markers() {
        let mappings = IndexMap::from([
            (
                "typing-extensions".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "typing-extensions".to_string(),
                    conda_versions: IndexMap::from([(
                        "4.0.0".to_string(),
                        "typing-extensions".to_string(),
                    )]),
                },
            ),
            (
                "pyobjc-core".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "pyobjc-core".to_string(),
                    conda_versions: IndexMap::from([(
                        "9.0".to_string(),
                        "pyobjc-core".to_string(),
                    )]),
                },
            ),
        ]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        // Test various system markers - all should be excluded for NoArch
        let test_cases = vec![
            ("typing-extensions; sys_platform == 'linux'", "sys_platform"),
            (
                "typing-extensions; platform_system == 'Linux'",
                "platform_system",
            ),
            ("typing-extensions; os_name == 'posix'", "os_name"),
            (
                "typing-extensions; platform_machine == 'x86_64'",
                "platform_machine",
            ),
            (
                "pyobjc-core; sys_platform == 'darwin'",
                "sys_platform darwin",
            ),
        ];

        for (req_str, marker_desc) in test_cases {
            let requirements = vec![pep508_rs::Requirement::from_str(req_str).unwrap()];

            let mapped = mapper
                .map_requirements(&requirements, Platform::NoArch)
                .await
                .unwrap();

            assert_eq!(
                mapped.len(),
                0,
                "NoArch should exclude dependency with {} marker",
                marker_desc
            );
        }
    }

    #[tokio::test]
    async fn test_noarch_includes_no_marker_dependencies() {
        let mappings = IndexMap::from([
            (
                "requests".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "requests".to_string(),
                    conda_versions: IndexMap::from([(
                        "2.31.0".to_string(),
                        "requests".to_string(),
                    )]),
                },
            ),
            (
                "flask".to_string(),
                PyPiPackageLookup {
                    format_version: "1".to_string(),
                    channel: "conda-forge".to_string(),
                    pypi_name: "flask".to_string(),
                    conda_versions: IndexMap::from([("2.0.0".to_string(), "flask".to_string())]),
                },
            ),
        ]);

        let mapper = PyPiToCondaMapper::with_inline_mappings(mappings);

        // Dependencies without markers should be included for NoArch
        let requirements = vec![
            pep508_rs::Requirement::from_str("requests>=2.0").unwrap(),
            pep508_rs::Requirement::from_str("flask").unwrap(),
        ];

        let mapped = mapper
            .map_requirements(&requirements, Platform::NoArch)
            .await
            .unwrap();

        assert_eq!(
            mapped.len(),
            2,
            "NoArch should include all dependencies without markers"
        );
        assert_eq!(mapped[0].name.as_normalized(), "requests");
        assert_eq!(mapped[1].name.as_normalized(), "flask");
    }
}
