//! Type-erased `provide`/`request` for extracting auxiliary data from
//! [`AnyKey`](crate::AnyKey)s without downcasting.
//!
//! This is a tiny reimplementation of the removed `std::any::Provider`
//! API, tailored to [`Key`](crate::Key)s. Override
//! [`Key::provide`](crate::Key::provide) to expose auxiliary values a
//! Key wants to make visible through the type-erased
//! [`AnyKey`](crate::AnyKey) surface.
//!
//! # Lifetime parameters
//!
//! [`Demand`] carries two lifetimes because the ref lifetime `'r`
//! (what a Key can provide via [`Demand::provide_ref`]) and the
//! slot-borrow lifetime `'s` (how long the caller's result slot is
//! alive) cannot share a single parameter without forcing one to
//! outlive the other at every call site. A single lifetime would
//! force the caller's stack-local slot to live as long as the Key's
//! `&self` borrow, which prevents the caller from reading the slot
//! out after `provide` returns. Splitting them keeps the slot
//! borrow scoped to the `request_*` call while letting the ref
//! lifetime match the Key's own.
//!
//! # Example
//!
//! ```ignore
//! impl Key for SourceRecordKey {
//!     // ...
//!     fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
//!         demand.provide_value(self.environment);
//!     }
//! }
//!
//! let any_key: AnyKey = /* from the cycle path */;
//! if let Some(env) = any_key.request_value::<CycleEnvironment>() {
//!     render_env(env);
//! }
//! ```

use std::{any::TypeId, marker::PhantomData};

/// A type-erased "slot" a caller creates to ask a Key for a specific
/// type. The Key's [`provide`](crate::Key::provide) impl fills the slot
/// only if the requested type matches what the Key wants to expose.
///
/// See the module docs for the meaning of the lifetime parameters.
pub struct Demand<'r, 's> {
    requested: TypeId,
    /// Raw pointer to the caller's result slot. The concrete type is
    /// determined by `requested`:
    ///
    /// - for [`Self::new_value`]: `*mut Option<T>`
    /// - for [`Self::new_ref`]:   `*mut Option<&'r T>`
    slot: *mut (),
    /// Invariant-in-`'r` marker that deliberately does not imply a
    /// borrow, so `'r` and `'s` can differ at the call site.
    _ref_lifetime: PhantomData<fn(&'r ()) -> &'r ()>,
    /// Mutable-borrow marker. Keeping the slot borrow in a separate
    /// lifetime is what lets the caller read the slot out after
    /// `provide` returns.
    _slot_borrow: PhantomData<&'s mut ()>,
}

impl<'r, 's> Demand<'r, 's> {
    /// Build a Demand whose slot accepts an owned `T`.
    pub(crate) fn new_value<T: 'static>(slot: &'s mut Option<T>) -> Self {
        Self {
            requested: TypeId::of::<T>(),
            slot: (slot as *mut Option<T>).cast(),
            _ref_lifetime: PhantomData,
            _slot_borrow: PhantomData,
        }
    }

    /// Build a Demand whose slot accepts a `&'r T` reference.
    pub(crate) fn new_ref<T: ?Sized + 'static>(slot: &'s mut Option<&'r T>) -> Self {
        Self {
            requested: TypeId::of::<T>(),
            slot: (slot as *mut Option<&'r T>).cast(),
            _ref_lifetime: PhantomData,
            _slot_borrow: PhantomData,
        }
    }

    /// Provide `value` if the caller requested `T`. If the requested
    /// type does not match, this is a no-op and `value` is dropped.
    pub fn provide_value<T: 'static>(&mut self, value: T) {
        if self.requested == TypeId::of::<T>() {
            // SAFETY: the TypeId match proves the slot was created
            // as `Option<T>`, and the `'s` bound keeps the slot
            // alive for as long as this Demand exists.
            unsafe {
                *(self.slot as *mut Option<T>) = Some(value);
            }
        }
    }

    /// Lazy variant of [`provide_value`](Self::provide_value).
    pub fn provide_value_with<T: 'static, F: FnOnce() -> T>(&mut self, f: F) {
        if self.requested == TypeId::of::<T>() {
            unsafe {
                *(self.slot as *mut Option<T>) = Some(f());
            }
        }
    }

    /// Provide a reference if the caller requested `&T`.
    pub fn provide_ref<T: ?Sized + 'static>(&mut self, value: &'r T) {
        if self.requested == TypeId::of::<T>() {
            unsafe {
                *(self.slot as *mut Option<&'r T>) = Some(value);
            }
        }
    }

    /// Lazy variant of [`provide_ref`](Self::provide_ref).
    pub fn provide_ref_with<T: ?Sized + 'static, F: FnOnce() -> &'r T>(&mut self, f: F) {
        if self.requested == TypeId::of::<T>() {
            unsafe {
                *(self.slot as *mut Option<&'r T>) = Some(f());
            }
        }
    }
}
