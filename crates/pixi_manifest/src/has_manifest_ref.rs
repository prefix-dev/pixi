/// Trait for types that have a reference to the original pixi manifest
/// Implement this along with [`crate::HasFeaturesIter`] to get an automatic
/// [`crate::FeaturesExt`] implementation
pub trait HasManifestRef<'source> {
    /// Returns access to the original pixi manifest
    fn manifest(&self) -> &'source crate::Manifest;
}

impl<'source> HasManifestRef<'source> for &'source crate::Manifest {
    fn manifest(&self) -> &'source crate::Manifest {
        self
    }
}
