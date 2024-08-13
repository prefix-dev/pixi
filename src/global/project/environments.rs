/// The environments in the global project.
#[derive(Debug, Clone, Default)]
pub struct Environments {
    /// A list of all environments, in the order they are defined in the
    /// manifest.
    pub(crate) environments: Vec<Option<Environment>>,

    /// A map of all environments, indexed by their name.
    pub(crate) by_name: IndexMap<EnvironmentName, EnvironmentIdx>,
}
