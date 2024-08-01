pub trait HasManifestRef<'m> {
    /// Returns access to the original pixi manifest
    fn manifest(&self) -> &'m crate::Manifest;
}
