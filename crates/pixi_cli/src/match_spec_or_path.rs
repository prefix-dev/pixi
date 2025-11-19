use std::{
    env,
    path::{Component, Path},
    str::FromStr,
};

use dunce::canonicalize;
use pixi_spec::PathSpec;
use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness, package::ArchiveIdentifier};

/// Represents either a regular conda MatchSpec or a filesystem path to a conda artifact.
#[derive(Debug, Clone)]
pub enum MatchSpecOrPath {
    MatchSpec(Box<MatchSpec>),
    Path(PathSpec),
}

impl MatchSpecOrPath {
    pub fn as_match_spec(&self) -> Option<&MatchSpec> {
        if let Self::MatchSpec(spec) = self {
            Some(spec.as_ref())
        } else {
            None
        }
    }

    pub fn is_path(&self) -> bool {
        matches!(self, Self::Path(_))
    }

    pub fn display_name(&self) -> Option<String> {
        match self {
            Self::MatchSpec(spec) => spec
                .name
                .as_ref()
                .map(|name| name.as_normalized().to_string()),
            Self::Path(path_spec) => path_spec
                .path
                .file_name()
                .map(|fname| fname.to_string())
                .or_else(|| Some(path_spec.path.as_str().to_string())),
        }
    }

    /// Convert into a MatchSpec suitable for execution, turning paths into file URLs.
    pub fn into_exec_match_spec(self) -> Result<MatchSpec, String> {
        match self {
            Self::MatchSpec(spec) => Ok(*spec),
            Self::Path(path_spec) => path_spec_to_match_spec(path_spec),
        }
    }

    /// Returns the underlying PathSpec, if any.
    pub fn into_path_spec(self) -> Result<PathSpec, String> {
        match self {
            Self::Path(path) => Ok(path),
            Self::MatchSpec(_) => Err("expected a path dependency".into()),
        }
    }
}

impl FromStr for MatchSpecOrPath {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // Check if this is a URL pointing to a conda package
        // Rattler's MatchSpec parser doesn't recognize URLs with schemes, so we handle them here
        if let Ok(url) = url::Url::parse(value) {
            if let Some(archive) = ArchiveIdentifier::try_from_url(&url) {
                // This is a URL to a conda package
                let name = PackageName::try_from(archive.name)
                    .map_err(|e| format!("invalid package name: {e}"))?;

                return Ok(Self::MatchSpec(Box::new(MatchSpec {
                    name: Some(name),
                    url: Some(url),
                    ..MatchSpec::default()
                })));
            }
        }

        match MatchSpec::from_str(value, ParseStrictness::Lenient) {
            Ok(spec) => Ok(Self::MatchSpec(Box::new(spec))),
            Err(parse_err) => {
                if looks_like_path(value) {
                    let path_spec = build_path_spec(value)?;
                    Ok(Self::Path(path_spec))
                } else {
                    Err(parse_err.to_string())
                }
            }
        }
    }
}

fn build_path_spec(value: &str) -> Result<PathSpec, String> {
    let provided = Path::new(value);
    let joined = if provided.is_absolute() {
        provided.to_path_buf()
    } else {
        let cwd = env::current_dir()
            .map_err(|err| format!("failed to determine current directory: {err}"))?;
        cwd.join(provided)
    };

    // Use canonical path when available to avoid duplicate cache keys, but fall back silently.
    let absolute = canonicalize(&joined).unwrap_or(joined);
    let path_str = absolute
        .to_str()
        .ok_or_else(|| format!("path '{}' is not valid UTF-8", absolute.display()))?;

    Ok(PathSpec::new(path_str.to_string()))
}

fn looks_like_path(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }

    if value.contains("::") {
        return false;
    }

    let path = Path::new(value);
    if path.is_absolute() {
        return true;
    }

    let mut components = path.components();
    let Some(first) = components.next() else {
        return false;
    };

    let starts_with_dot = matches!(first, Component::CurDir | Component::ParentDir);
    let has_multiple_components = components.next().is_some();
    let looks_like_archive = value.ends_with(".conda") || value.ends_with(".tar.bz2");

    starts_with_dot
        || has_multiple_components
        || value.contains(std::path::MAIN_SEPARATOR)
        || value.contains('/')
        || value.contains('\\')
        || looks_like_archive
}

fn path_spec_to_match_spec(path_spec: PathSpec) -> Result<MatchSpec, String> {
    let path = Path::new(path_spec.path.as_str());

    // Invariant for if we ever change stuff around
    debug_assert!(
        path.is_absolute(),
        "path_spec_to_match_spec expects absolute paths"
    );

    let url = url::Url::from_file_path(path)
        .map_err(|_| format!("failed to convert '{}' into a file:// url", path.display()))?;

    // Extract package name from the archive
    let archive = ArchiveIdentifier::try_from_url(&url)
        .ok_or_else(|| format!("failed to parse package archive from '{url}'"))?;

    let name =
        PackageName::try_from(archive.name).map_err(|e| format!("invalid package name: {e}"))?;

    Ok(MatchSpec {
        name: Some(name),
        url: Some(url),
        ..MatchSpec::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_relative_like_inputs() {
        assert!(looks_like_path("./pkg/file.conda"));
        assert!(looks_like_path("pkg/file.conda"));
        assert!(looks_like_path("file.tar.bz2"));
        assert!(looks_like_path("file.conda"));
        assert!(!looks_like_path("python>=3.12"));
        assert!(!looks_like_path("conda-forge::python"));
    }

    #[test]
    fn parses_https_url() {
        let result = MatchSpecOrPath::from_str(
            "https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda",
        );
        assert!(result.is_ok(), "Failed to parse HTTPS URL: {:?}", result);
        let spec_or_path = result.unwrap();
        match spec_or_path {
            MatchSpecOrPath::MatchSpec(spec) => {
                assert_eq!(
                    spec.name.as_ref().map(|n| n.as_normalized()),
                    Some("tzdata")
                );
                assert!(spec.url.is_some());
            }
            _ => panic!("Expected MatchSpec, got Path"),
        }
    }

    #[test]
    fn parses_file_url() {
        let result = MatchSpecOrPath::from_str("file:///tmp/test-package-1.0.0-h123_0.conda");
        assert!(result.is_ok());
        let spec_or_path = result.unwrap();
        match spec_or_path {
            MatchSpecOrPath::MatchSpec(spec) => {
                assert_eq!(
                    spec.name.as_ref().map(|n| n.as_normalized()),
                    Some("test-package")
                );
                assert!(spec.url.is_some());
                assert_eq!(spec.url.as_ref().unwrap().scheme(), "file");
            }
            _ => panic!("Expected MatchSpec, got Path"),
        }
    }
}
