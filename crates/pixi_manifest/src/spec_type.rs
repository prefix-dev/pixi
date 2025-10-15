#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
/// What kind of dependency spec do we have
pub enum SpecType {
    /// Dev dependencies are used during development of the package itself
    Dev,
    /// Host dependencies are used that are needed by the host environment when
    /// running the project
    Host,
    /// Build dependencies are used when we need to build the project, may not
    /// be required at runtime
    Build,
    /// Regular dependencies that are used when we need to run the project
    Run,
}

impl SpecType {
    /// Convert to a name used in the manifest
    pub fn name(&self) -> &'static str {
        match self {
            SpecType::Dev => "dev-dependencies",
            SpecType::Host => "host-dependencies",
            SpecType::Build => "build-dependencies",
            SpecType::Run => "dependencies",
        }
    }

    /// Returns all the variants of the enum
    pub fn all() -> impl Iterator<Item = SpecType> {
        [
            SpecType::Run,
            SpecType::Dev,
            SpecType::Host,
            SpecType::Build,
        ]
        .into_iter()
    }
}
