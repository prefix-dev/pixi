use crate::Project;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Version, VersionBumpType};
use std::str::FromStr;

pub async fn execute(mut project: Project, bump_type: VersionBumpType) -> miette::Result<()> {
    // get version and exit with error if not found
    let current_version = project
        .version()
        .as_ref()
        .ok_or_else(|| miette::miette!("No version found in manifest."))?
        .clone();

    // First extend the version to ensure we have all segments (major.minor.patch)
    let current_version = current_version
        .extend_to_length(3)
        .into_diagnostic()
        .context("Failed to extend version to major.minor.patch format")?;

    // bump version
    let mut new_version = current_version
        .bump(bump_type.clone())
        .into_diagnostic()
        .context("Failed to bump version.")?;

    // Reset lower segments based on bump type
    let version_str = match bump_type {
        VersionBumpType::Major => {
            // For major bump, reset both minor and patch to 0
            let major = new_version
                .segments()
                .next()
                .and_then(|s| s.components().next())
                .ok_or_else(|| miette::miette!("Invalid version format"))?;
            format!("{}.0.0", major)
        }
        VersionBumpType::Minor => {
            // For minor bump, reset patch to 0
            let major = new_version
                .segments()
                .next()
                .and_then(|s| s.components().next())
                .ok_or_else(|| miette::miette!("Invalid version format"))?;
            let minor = new_version
                .segments()
                .nth(1)
                .and_then(|s| s.components().next())
                .ok_or_else(|| miette::miette!("Invalid version format"))?;
            format!("{}.{}.0", major, minor)
        }
        _ => new_version.to_string(), // For patch bump, no reset needed
    };

    // Parse the new version string
    new_version = Version::from_str(&version_str)
        .into_diagnostic()
        .context("Failed to parse new version")?;

    // Set the version
    project.manifest.set_version(&new_version.to_string())?;

    // Save the manifest on disk
    project.save()?;

    // Report back to the user
    eprintln!(
        "{}Updated project version from '{}' to '{}'.",
        console::style(console::Emoji("✔ ", "")).green(),
        current_version,
        new_version,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::Version;
    use std::str::FromStr;

    fn bump_version(version: &str, bump_type: VersionBumpType) -> String {
        let current_version = Version::from_str(version).unwrap();
        let current_version = current_version.extend_to_length(3).unwrap();

        let new_version = current_version.bump(bump_type.clone()).unwrap();

        match bump_type {
            VersionBumpType::Major => {
                let major = new_version
                    .segments()
                    .next()
                    .and_then(|s| s.components().next())
                    .unwrap();
                format!("{}.0.0", major)
            }
            VersionBumpType::Minor => {
                let major = new_version
                    .segments()
                    .next()
                    .and_then(|s| s.components().next())
                    .unwrap();
                let minor = new_version
                    .segments()
                    .nth(1)
                    .and_then(|s| s.components().next())
                    .unwrap();
                format!("{}.{}.0", major, minor)
            }
            _ => new_version.to_string(),
        }
    }

    #[test]
    fn test_major_version_bump() {
        assert_eq!(bump_version("1.2.3", VersionBumpType::Major), "2.0.0");
        assert_eq!(bump_version("0.1.0", VersionBumpType::Major), "1.0.0");
        assert_eq!(bump_version("1.0.0", VersionBumpType::Major), "2.0.0");
    }

    #[test]
    fn test_minor_version_bump() {
        assert_eq!(bump_version("1.2.3", VersionBumpType::Minor), "1.3.0");
        assert_eq!(bump_version("1.9.0", VersionBumpType::Minor), "1.10.0");
        assert_eq!(bump_version("0.1.0", VersionBumpType::Minor), "0.2.0");
    }

    #[test]
    fn test_patch_version_bump() {
        assert_eq!(bump_version("1.2.3", VersionBumpType::Patch), "1.2.4");
        assert_eq!(bump_version("1.2.9", VersionBumpType::Patch), "1.2.10");
        assert_eq!(bump_version("0.1.0", VersionBumpType::Patch), "0.1.1");
    }

    #[test]
    fn test_incomplete_version_bump() {
        assert_eq!(bump_version("1", VersionBumpType::Major), "2.0.0");
        assert_eq!(bump_version("1", VersionBumpType::Minor), "1.1.0");
        assert_eq!(bump_version("1.1", VersionBumpType::Major), "2.0.0");
        assert_eq!(bump_version("1.1", VersionBumpType::Minor), "1.2.0");
    }
}
