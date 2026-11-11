//! Miscellaneous `Key`-trait default-behavior tests.

use pixi_compute_engine::Key;

use super::common::{BaseKey, DoubleKey, PlusTenKey};

/// `key_type_name` returns the bare type name without its module path.
#[test]
fn key_type_name_strips_module_path() {
    assert_eq!(BaseKey::key_type_name(), "BaseKey");
    assert_eq!(PlusTenKey::key_type_name(), "PlusTenKey");
    assert_eq!(DoubleKey::key_type_name(), "DoubleKey");
}

/// The default `Key::equality` is `false`: values never compare equal
/// unless a Key overrides it.
#[test]
fn default_equality_is_false() {
    assert!(!DoubleKey::equality(&42, &42));
    assert!(!BaseKey::equality(&42, &42));
}
