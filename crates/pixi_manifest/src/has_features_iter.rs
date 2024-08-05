use crate::Feature;

/// This trait is implemented by types that contain a collection of Features.
/// So that an abstraction can be made over these features and de-coupled from how
/// they are actually provided
///
/// Implement this along with [`crate::HasManifestRef`] to get an automatic
/// [`crate::FeaturesExt`] implementation
pub trait HasFeaturesIter<'source> {
    /// Returns an iterator to all Features in this collection
    fn features(&self) -> impl DoubleEndedIterator<Item = &'source Feature> + 'source;
}
