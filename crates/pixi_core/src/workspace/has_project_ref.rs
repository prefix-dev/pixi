use crate::Workspace;

/// Trait for objects that have a reference to a Project.
pub trait HasWorkspaceRef<'p> {
    /// Reference to the project.
    fn workspace(&self) -> &'p Workspace;
}
