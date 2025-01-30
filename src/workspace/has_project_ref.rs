use crate::Workspace;

/// Trait for objects that have a reference to a Project.
pub trait HasProjectRef<'p> {
    /// Reference to the project.
    fn project(&self) -> &'p Workspace;
}
