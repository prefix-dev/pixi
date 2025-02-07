use crate::{AssociateProvenance, ManifestKind, WithProvenance};
use miette::{NamedSource, SourceCode};

/// Discriminates the source of between a 'pixi.toml' and a 'pyproject.toml'
/// manifest.
pub enum ManifestSource<S> {
    PyProjectToml(S),
    PixiToml(S),
}

impl<S> AsRef<S> for ManifestSource<S> {
    fn as_ref(&self) -> &S {
        match self {
            ManifestSource::PyProjectToml(source) => source,
            ManifestSource::PixiToml(source) => source,
        }
    }
}

impl<S> ManifestSource<S> {
    /// Returns the inner source of the manifest.
    pub fn into_inner(self) -> S {
        match self {
            ManifestSource::PyProjectToml(source) => source,
            ManifestSource::PixiToml(source) => source,
        }
    }

    /// Returns the kind of manifest this source represents.
    pub fn kind(&self) -> ManifestKind {
        match self {
            ManifestSource::PyProjectToml(_) => ManifestKind::Pyproject,
            ManifestSource::PixiToml(_) => ManifestKind::Pixi,
        }
    }

    /// Maps the source from one type to another.
    pub fn map<U, F: FnOnce(S) -> U>(self, f: F) -> ManifestSource<U> {
        match self {
            ManifestSource::PyProjectToml(source) => ManifestSource::PyProjectToml(f(source)),
            ManifestSource::PixiToml(source) => ManifestSource::PixiToml(f(source)),
        }
    }

    /// Turns this instance into a [`WithProvenance`] where the provenance is
    /// derived from the type of the manifest.
    pub fn with_provenance_from_kind(self) -> WithProvenance<S> {
        let kind = self.kind();
        self.into_inner().with_provenance(kind.into())
    }
}

impl<S: SourceCode + 'static> ManifestSource<S> {
    /// Converts this instance into a [`NamedSource`] with the appropriate name
    /// set based on the type of manifest.
    pub fn into_named(self, file_name: impl AsRef<str>) -> NamedSource<S> {
        NamedSource::new(file_name, self.into_inner()).with_language("toml")
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;
    use rstest::rstest;

    use super::*;
    use crate::manifests::document::ManifestDocument;
    use crate::{
        FeatureName, LibCFamilyAndVersion, LibCSystemRequirement, ManifestProvenance, Manifests,
        SystemRequirements,
    };

    #[rstest]
    #[case::pixi_toml(ManifestDocument::empty_pixi())]
    #[case::pyproject_toml(ManifestDocument::empty_pyproject())]
    fn test_add_environment(#[case] mut source: ManifestDocument) {
        source
            .add_environment("foo", Some(vec![]), None, false)
            .unwrap();
        source
            .add_environment("bar", Some(vec![String::from("default")]), None, false)
            .unwrap();
        source
            .add_environment(
                "baz",
                Some(vec![String::from("default")]),
                Some(String::from("group1")),
                false,
            )
            .unwrap();
        source
            .add_environment(
                "foobar",
                Some(vec![String::from("default")]),
                Some(String::from("group1")),
                true,
            )
            .unwrap();
        source
            .add_environment("barfoo", Some(vec![String::from("default")]), None, true)
            .unwrap();

        // Overwrite
        source
            .add_environment("bar", Some(vec![String::from("not-default")]), None, false)
            .unwrap();

        assert_snapshot!(
            format!("test_add_environment_{}", source.file_name()),
            source.to_string()
        );
    }

    #[rstest]
    #[case::pixi_toml(ManifestDocument::empty_pixi())]
    #[case::pyproject_toml(ManifestDocument::empty_pyproject())]
    fn test_remove_environment(#[case] mut source: ManifestDocument) {
        source
            .add_environment("foo", Some(vec![String::from("default")]), None, false)
            .unwrap();
        source
            .add_environment("bar", Some(vec![String::from("default")]), None, false)
            .unwrap();
        assert!(!source.remove_environment("default").unwrap());
        source
            .add_environment("default", Some(vec![String::from("default")]), None, false)
            .unwrap();
        assert!(source.remove_environment("default").unwrap());
        assert!(source.remove_environment("foo").unwrap());
        assert_snapshot!(
            format!("test_remove_environment_{}", source.file_name()),
            source.to_string()
        );
    }

    #[rstest]
    #[case::pixi_toml(ManifestDocument::empty_pixi())]
    #[case::pyproject_toml(ManifestDocument::empty_pyproject())]
    fn test_add_empty_system_requirement_environment(#[case] mut source: ManifestDocument) {
        let empty_requirements = SystemRequirements::default();
        source
            .add_system_requirements(&empty_requirements, &FeatureName::Default)
            .unwrap();

        let manifests = Manifests::from_workspace_source(source.into_source_with_provenance())
            .unwrap()
            .value;

        assert_eq!(
            empty_requirements,
            manifests
                .workspace
                .value
                .default_feature()
                .system_requirements
        );
    }
    #[rstest]
    #[case::pixi_toml(ManifestDocument::empty_pixi())]
    #[case::pyproject_toml(ManifestDocument::empty_pyproject())]
    fn test_add_single_system_requirement_environment(#[case] mut source: ManifestDocument) {
        let single_system_requirements = SystemRequirements {
            linux: Some("4.18".parse().unwrap()),
            ..SystemRequirements::default()
        };
        source
            .add_system_requirements(&single_system_requirements, &FeatureName::Default)
            .unwrap();

        let manifests = Manifests::from_workspace_source(source.into_source_with_provenance())
            .unwrap()
            .value;

        assert_eq!(
            single_system_requirements,
            manifests
                .workspace
                .value
                .default_feature()
                .system_requirements
        );
    }
    #[rstest]
    #[case::pixi_toml(ManifestDocument::empty_pixi())]
    #[case::pyproject_toml(ManifestDocument::empty_pyproject())]
    fn test_add_full_system_requirement_environment(#[case] mut source: ManifestDocument) {
        let full_system_requirements = SystemRequirements {
            linux: Some("4.18".parse().unwrap()),
            cuda: Some("11.1".parse().unwrap()),
            macos: Some("13.0".parse().unwrap()),
            libc: Some(LibCSystemRequirement::GlibC("2.28".parse().unwrap())),
            archspec: Some("x86_64".to_string()),
        };
        source
            .add_system_requirements(&full_system_requirements, &FeatureName::Default)
            .unwrap();

        let kind = source.kind();
        let source = source.to_string();
        let manifests = Manifests::from_workspace_source(
            source
                .as_str()
                .with_provenance(ManifestProvenance::from(kind)),
        )
        .unwrap()
        .value;

        assert_eq!(
            full_system_requirements,
            manifests
                .workspace
                .value
                .default_feature()
                .system_requirements
        );
        assert_snapshot!(
            format!(
                "test_add_full_system_requirement_environment_{}",
                manifests.workspace.provenance.path.display()
            ),
            source.to_string()
        );
    }
    #[rstest]
    #[case::pixi_toml(ManifestDocument::empty_pixi())]
    #[case::pyproject_toml(ManifestDocument::empty_pyproject())]
    fn test_add_libc_family_system_requirement_environment(#[case] mut source: ManifestDocument) {
        let family_system_requirements = SystemRequirements {
            libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                family: Some("glibc".to_string()),
                version: "1.2".parse().unwrap(),
            })),
            ..SystemRequirements::default()
        };
        source
            .add_system_requirements(&family_system_requirements, &FeatureName::Default)
            .unwrap();

        let kind = source.kind();
        let source = source.to_string();
        let manifests = Manifests::from_workspace_source(
            source
                .as_str()
                .with_provenance(ManifestProvenance::from(kind)),
        )
        .unwrap()
        .value;

        assert_eq!(
            family_system_requirements,
            manifests
                .workspace
                .value
                .default_feature()
                .system_requirements
        );

        assert_snapshot!(
            format!(
                "test_add_family_system_requirement_environment_{}",
                manifests.workspace.provenance.path.display()
            ),
            source.to_string()
        );
    }
}
