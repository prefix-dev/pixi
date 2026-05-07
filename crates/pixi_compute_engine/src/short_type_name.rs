//! Short, log-friendly type names.
//!
//! [`std::any::type_name`] returns a fully qualified path. For log output and
//! error messages we usually just want the final segment. This module
//! provides a tiny helper that does that.
//!
//! Types containing generics (detected by a `>` in the final segment) are
//! returned unchanged, since naively splitting on `::` would cut generic
//! arguments in half.

/// Returns a shortened version of [`std::any::type_name`] for `T`.
///
/// For a plain type like `some_crate::module::Foo`, returns `"Foo"`. For a
/// generic type like `some_crate::module::Outer<Inner>`, returns the full
/// name unchanged (shortening generic arguments is a TODO).
///
/// This is used by the default implementation of [`Key::key_type_name`]
/// to produce log-friendly labels for [`AnyKey`] renderings.
///
/// [`Key::key_type_name`]: crate::Key::key_type_name
/// [`AnyKey`]: crate::AnyKey
///
/// # Example
///
/// ```
/// use pixi_compute_engine::short_type_name;
///
/// struct Widget;
/// assert_eq!(short_type_name::<Widget>(), "Widget");
/// assert_eq!(short_type_name::<u32>(), "u32");
/// ```
pub fn short_type_name<T: ?Sized>() -> &'static str {
    shorten(std::any::type_name::<T>())
}

fn shorten(type_name: &str) -> &str {
    type_name
        .rsplit("::")
        .next()
        .filter(|s| !s.contains('>'))
        .unwrap_or(type_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_outer_path() {
        assert_eq!(shorten("some_crate::some_mod::Foo"), "Foo");
    }

    #[test]
    fn preserves_generics_unchanged() {
        assert_eq!(
            shorten("a::b::Outer<c::d::Inner>"),
            "a::b::Outer<c::d::Inner>"
        );
    }

    #[test]
    fn handles_no_path() {
        assert_eq!(shorten("Foo"), "Foo");
    }

    #[test]
    fn returns_static_from_type_name() {
        struct Local;
        let name: &'static str = short_type_name::<Local>();
        assert!(name.ends_with("Local"));
    }
}
