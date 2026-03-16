/// Trait for types that have a reference to workspace manifest.
pub trait HasWorkspaceManifest<'source> {
    fn workspace_manifest(&self) -> &'source crate::WorkspaceManifest;
}
