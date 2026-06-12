use crate::{AssociateProvenance, ManifestKind, WithProvenance};
use miette::{NamedSource, SourceCode};

/// Discriminates the source of between a 'pixi.toml' and a 'pyproject.toml'
/// manifest.
pub enum ManifestSource<S> {
    PyProjectToml(S),
    PixiToml(S),
    MojoProjectToml(S),
}

impl<S> AsRef<S> for ManifestSource<S> {
    fn as_ref(&self) -> &S {
        match self {
            ManifestSource::PyProjectToml(source) => source,
            ManifestSource::PixiToml(source) => source,
            ManifestSource::MojoProjectToml(source) => source,
        }
    }
}

impl<S> ManifestSource<S> {
    /// Returns the inner source of the manifest.
    pub fn into_inner(self) -> S {
        match self {
            ManifestSource::PyProjectToml(source) => source,
            ManifestSource::PixiToml(source) => source,
            ManifestSource::MojoProjectToml(source) => source,
        }
    }

    /// Returns the kind of manifest this source represents.
    pub fn kind(&self) -> ManifestKind {
        match self {
            ManifestSource::PyProjectToml(_) => ManifestKind::Pyproject,
            ManifestSource::PixiToml(_) => ManifestKind::Pixi,
            ManifestSource::MojoProjectToml(_) => ManifestKind::MojoProject,
        }
    }

    /// Maps the source from one type to another.
    pub fn map<U, F: FnOnce(S) -> U>(self, f: F) -> ManifestSource<U> {
        match self {
            ManifestSource::PyProjectToml(source) => ManifestSource::PyProjectToml(f(source)),
            ManifestSource::PixiToml(source) => ManifestSource::PixiToml(f(source)),
            ManifestSource::MojoProjectToml(source) => ManifestSource::MojoProjectToml(f(source)),
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

    use crate::manifests::document::ManifestDocument;

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
}
