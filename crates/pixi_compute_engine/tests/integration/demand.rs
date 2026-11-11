//! Round-trip tests for `Key::provide` + `AnyKey::request_value` /
//! `AnyKey::request_ref`.
//!
//! These tests exercise the `unsafe` path in `src/demand.rs` end to
//! end: a Key stores values into a `Demand`'s slot via `provide_value`
//! / `provide_ref`, and `AnyKey` reads them back out. They are the
//! only coverage of the type-erased `Demand` machinery.
//!
//! See also the module-level docs in `src/demand.rs` for the lifetime
//! design (`Demand<'r, 's>`) and why the two parameters are needed.

use derive_more::Display;
use pixi_compute_engine::{AnyKey, ComputeCtx, Demand, Key};

/// A Key that provides an owned value of type `u32` via
/// `provide_value` and a `&'static str` via `provide_ref`. The
/// `'static` label lets us ignore the `'r` lifetime for the
/// reference case.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{value}/{label}")]
struct ProviderKey {
    value: u32,
    label: &'static str,
}

impl Key for ProviderKey {
    type Value = ();
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {}

    fn provide<'a>(&'a self, demand: &mut Demand<'a, '_>) {
        demand.provide_value(self.value);
        // Using `&'static str` here keeps the test focused on the
        // owned-value vs reference distinction without also
        // exercising the `'r = &self` borrow lifetime; that case has
        // its own test (`provide_ref_to_key_field`).
        demand.provide_ref::<str>(self.label);
    }
}

/// A Key that doesn't override `provide`, serving as a negative
/// control for the default no-op behavior.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("silent")]
struct SilentKey;

impl Key for SilentKey {
    type Value = ();
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {}
}

/// `request_value::<T>` returns the value the Key provided.
#[test]
fn request_value_round_trip() {
    let k = AnyKey::new(ProviderKey {
        value: 42,
        label: "hello",
    });
    assert_eq!(k.request_value::<u32>(), Some(42));
}

/// `request_ref::<T>` returns the reference the Key provided.
#[test]
fn request_ref_round_trip() {
    let k = AnyKey::new(ProviderKey {
        value: 42,
        label: "hello",
    });
    assert_eq!(k.request_ref::<str>(), Some("hello"));
}

/// Requesting a type the Key does NOT provide returns `None` even
/// when the Key provides other types.
#[test]
fn request_type_mismatch_returns_none() {
    let k = AnyKey::new(ProviderKey {
        value: 42,
        label: "hello",
    });
    // Asking for `String` when the Key provides `u32` and `&str`.
    assert_eq!(k.request_value::<String>(), None);
    // Asking for `[u8]` ref when the Key provides `str` ref.
    assert_eq!(k.request_ref::<[u8]>(), None);
}

/// A Key that does not override `provide` exposes no values.
#[test]
fn silent_key_provides_nothing() {
    let k = AnyKey::new(SilentKey);
    assert_eq!(k.request_value::<u32>(), None);
    assert_eq!(k.request_ref::<str>(), None);
}

/// The `provide_value_with` lazy variant only constructs its value
/// when the TypeId matches. We can't observe construction directly,
/// but we can verify correctness: a Key that uses `provide_value_with`
/// still round-trips the value.
#[test]
fn provide_value_with_lazy_variant_round_trips() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("lazy")]
    struct LazyKey;
    impl Key for LazyKey {
        type Value = ();
        async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {}
        fn provide<'a>(&'a self, demand: &mut Demand<'a, '_>) {
            demand.provide_value_with::<u64, _>(|| 12345);
        }
    }

    let k = AnyKey::new(LazyKey);
    assert_eq!(k.request_value::<u64>(), Some(12345));
    // Unmatched type must still return None so that `_with` does
    // not invoke a possibly-expensive closure speculatively.
    assert_eq!(k.request_value::<i64>(), None);
}

/// Providing multiple types and reading them back independently.
#[test]
fn multiple_types_provided_independently() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("multi")]
    struct MultiKey;
    impl Key for MultiKey {
        type Value = ();
        async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {}
        fn provide<'a>(&'a self, demand: &mut Demand<'a, '_>) {
            demand.provide_value::<u32>(1);
            demand.provide_value::<u64>(2);
            demand.provide_value::<i32>(-3);
        }
    }

    let k = AnyKey::new(MultiKey);
    assert_eq!(k.request_value::<u32>(), Some(1));
    assert_eq!(k.request_value::<u64>(), Some(2));
    assert_eq!(k.request_value::<i32>(), Some(-3));
    assert_eq!(k.request_value::<i64>(), None);
}

/// A Key that provides a reference to one of its own fields. This
/// specifically exercises the `'r = &self` lifetime in
/// `provide<'a>(&'a self, demand: &mut Demand<'a, '_>)`: the
/// reference must have at least the Key's borrow lifetime, not
/// just `'static`.
#[test]
fn provide_ref_to_key_field() {
    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
    #[display("{name}")]
    struct FieldKey {
        name: String,
    }
    impl Key for FieldKey {
        type Value = ();
        async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {}
        fn provide<'a>(&'a self, demand: &mut Demand<'a, '_>) {
            demand.provide_ref::<str>(&self.name);
        }
    }

    let k = AnyKey::new(FieldKey {
        name: "alpha".to_string(),
    });
    assert_eq!(k.request_ref::<str>(), Some("alpha"));
}
