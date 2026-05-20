//! ROS-specific metadata provider for package.xml files.
//!
//! Implements the `MetadataProvider` trait to extract package metadata from
//! package.xml and format names according to ROS conventions.

use std::collections::BTreeSet;

use miette::IntoDiagnostic;
use pixi_build_backend::generated_recipe::{GeneratedRecipe, MetadataProvider};
use pixi_build_types::ProjectModel;
use rattler_conda_types::Version;

use crate::package_xml::PackageXml;

/// Parse a [`PackageXml`] and render a [`GeneratedRecipe`] from the given
/// project model, applying ROS-specific metadata (name, version, license, etc.)
/// and collecting input globs.
pub fn parse_and_render(
    package_xml: PackageXml,
    distro_name: &str,
    model: ProjectModel,
    extra_input_globs: Vec<String>,
    package_mapping_files: Vec<String>,
) -> miette::Result<GeneratedRecipe> {
    let mut provider = RosPackageXmlMetadataProvider {
        package_data: package_xml,
        distro_name: distro_name.to_string(),
        extra_input_globs,
        package_mapping_files,
    };

    let mut recipe = GeneratedRecipe::from_model(model, &mut provider).into_diagnostic()?;
    recipe.metadata_input_globs.extend(provider.input_globs());
    Ok(recipe)
}

/// Metadata provider for ROS package.xml files.
///
/// Formats package names as `ros-<distro>-<name>` with underscore-to-hyphen
/// conversion.
#[derive(Debug)]
struct RosPackageXmlMetadataProvider {
    package_data: PackageXml,
    distro_name: String,
    extra_input_globs: Vec<String>,
    package_mapping_files: Vec<String>,
}

impl RosPackageXmlMetadataProvider {
    fn input_globs(&self) -> BTreeSet<String> {
        let mut globs: BTreeSet<String> = BTreeSet::from([
            "package.xml".to_string(),
            "CMakeLists.txt".to_string(),
            "setup.py".to_string(),
            "setup.cfg".to_string(),
        ]);
        for g in &self.extra_input_globs {
            globs.insert(g.clone());
        }
        for f in &self.package_mapping_files {
            globs.insert(f.clone());
        }
        globs
    }
}

impl MetadataProvider for RosPackageXmlMetadataProvider {
    type Error = std::convert::Infallible;

    fn name(&mut self) -> Result<Option<String>, Self::Error> {
        let formatted = self.package_data.name.replace('_', "-");
        Ok(Some(format!("ros-{}-{}", self.distro_name, formatted)))
    }

    fn version(&mut self) -> Result<Option<Version>, Self::Error> {
        Ok(self.package_data.version.parse::<Version>().ok())
    }

    fn homepage(&mut self) -> Result<Option<String>, Self::Error> {
        // First URL of type "website", or first URL with no type
        let homepage = self
            .package_data
            .urls
            .iter()
            .find(|u| u.url_type.as_deref() == Some("website"))
            .or_else(|| self.package_data.urls.iter().find(|u| u.url_type.is_none()))
            .or_else(|| self.package_data.urls.first())
            .map(|u| u.url.clone());
        Ok(homepage)
    }

    fn license(&mut self) -> Result<Option<String>, Self::Error> {
        if self.package_data.licenses.len() == 1 && !self.package_data.licenses[0].contains("TODO")
        {
            Ok(Some(format!(
                "LicenseRef-{}",
                self.package_data.licenses[0]
            )))
        } else {
            Ok(None)
        }
    }

    fn license_files(&mut self) -> Result<Option<Vec<String>>, Self::Error> {
        Ok(None)
    }

    fn summary(&mut self) -> Result<Option<String>, Self::Error> {
        match &self.package_data.description {
            Some(desc) if desc.len() > 100 => Ok(Some(format!("{}...", &desc[..97]))),
            Some(desc) => Ok(Some(desc.clone())),
            None => Ok(None),
        }
    }

    fn description(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.package_data.description.clone())
    }

    fn documentation(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }

    fn repository(&mut self) -> Result<Option<String>, Self::Error> {
        let repo = self
            .package_data
            .urls
            .iter()
            .find(|u| u.url_type.as_deref() == Some("repository"))
            .map(|u| u.url.clone());
        Ok(repo)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn package_xmls_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/package_xmls")
    }

    fn load_package_xml(name: &str) -> PackageXml {
        let path = package_xmls_dir().join(name);
        let content = fs_err::read_to_string(path).unwrap();
        PackageXml::parse(&content).unwrap()
    }

    #[test]
    fn test_metadata_provider() {
        let package_xml = load_package_xml("custom_ros.xml");
        let mut provider = RosPackageXmlMetadataProvider {
            package_data: package_xml,
            distro_name: "noetic".to_string(),
            extra_input_globs: Vec::new(),
            package_mapping_files: Vec::new(),
        };

        // The name is ROS-formatted
        assert_eq!(
            provider.name().unwrap().as_deref(),
            Some("ros-noetic-custom-ros")
        );
        assert_eq!(
            provider.version().unwrap().map(|v| v.to_string()),
            Some("0.0.1".to_string())
        );
        assert_eq!(
            provider.license().unwrap().as_deref(),
            Some("LicenseRef-Apache License 2.0")
        );
        assert_eq!(provider.description().unwrap().as_deref(), Some("Demo"));
        assert_eq!(
            provider.homepage().unwrap().as_deref(),
            Some("https://test.io/custom_ros")
        );
        assert_eq!(
            provider.repository().unwrap().as_deref(),
            Some("https://github.com/test/custom_ros")
        );
        assert_eq!(provider.license_files().unwrap(), None);
    }

    #[test]
    fn test_metadata_provider_input_globs_include_mapping_files() {
        let package_xml = load_package_xml("custom_ros.xml");
        let provider = RosPackageXmlMetadataProvider {
            package_data: package_xml,
            distro_name: "noetic".to_string(),
            extra_input_globs: Vec::new(),
            package_mapping_files: vec!["/tmp/custom_mapping.yaml".to_string()],
        };

        let globs = provider.input_globs();
        assert!(globs.contains("package.xml"));
        assert!(globs.contains("CMakeLists.txt"));
        assert!(globs.contains("/tmp/custom_mapping.yaml"));
    }
}
