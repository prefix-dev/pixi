// use uv_normalize::PackageName;
// use pep508_rs::PackageName;

use once_cell::sync::Lazy;
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Debug,
    str::FromStr,
    sync::{Arc, RwLock},
};

use dashmap::DashMap;

// use crate::to_uv_normalize;

// Create a globally accessible instance of `UvConversions` wrapped in `Arc`.
pub static GLOBAL_UV_CONVERSIONS: Lazy<Arc<UvConversions>> =
    Lazy::new(|| Arc::new(UvConversions::new()));

pub struct UvConversions {
    pep_to_uv_name: DashMap<pep508_rs::PackageName, uv_normalize::PackageName>,
    uv_to_pep_name: DashMap<uv_normalize::PackageName, pep508_rs::PackageName>,
    pep_to_uv_extra_name: DashMap<pep508_rs::ExtraName, uv_normalize::ExtraName>,
    uv_to_pep_extra_name: DashMap<uv_normalize::ExtraName, pep508_rs::ExtraName>,
}

impl Debug for UvConversions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // print the number of items in each map
        f.debug_struct("UvConversions")
            .field("pep_to_uv_name", &self.pep_to_uv_name.len())
            .field("uv_to_pep_name", &self.uv_to_pep_name.len())
            .field("pep_to_uv_extra_name", &self.pep_to_uv_extra_name.len())
            .field("uv_to_pep_extra_name", &self.uv_to_pep_extra_name.len())
            .finish()
    }
}

impl Default for UvConversions {
    fn default() -> Self {
        Self::new()
    }
}

impl UvConversions {
    pub fn new() -> Self {
        Self {
            pep_to_uv_name: DashMap::with_capacity(52),
            uv_to_pep_name: DashMap::with_capacity(52),
            pep_to_uv_extra_name: DashMap::with_capacity(52),
            uv_to_pep_extra_name: DashMap::with_capacity(52),
        }
    }

    pub fn to_uv_normalize(&self, name: &pep508_rs::PackageName) -> uv_normalize::PackageName {
        if let Some(cached_uv_name) = self.pep_to_uv_name.get(name) {
            return cached_uv_name.clone();
        }

        let uv_name = to_uv_normalize(name);
        self.pep_to_uv_name.insert(name.clone(), uv_name.clone());

        uv_name
    }

    pub fn to_normalize(&self, name: &uv_normalize::PackageName) -> pep508_rs::PackageName {
        if let Some(cached_uv_name) = self.uv_to_pep_name.get(name) {
            return cached_uv_name.clone();
        }

        let pep_name = to_normalize(name);
        self.uv_to_pep_name.insert(name.clone(), pep_name.clone());

        pep_name
    }

    pub fn to_uv_extra_name(&self, name: &pep508_rs::ExtraName) -> uv_normalize::ExtraName {
        if let Some(cached_uv_name) = self.pep_to_uv_extra_name.get(name) {
            return cached_uv_name.clone();
        }

        let pep_name = to_uv_extra_name(name);
        self.pep_to_uv_extra_name
            .insert(name.clone(), pep_name.clone());

        pep_name
    }

    pub fn to_extra_name(&self, name: &uv_normalize::ExtraName) -> pep508_rs::ExtraName {
        if let Some(cached_uv_name) = self.uv_to_pep_extra_name.get(name) {
            return cached_uv_name.clone();
        }

        let pep_name = to_extra_name(name);
        self.uv_to_pep_extra_name
            .insert(name.clone(), pep_name.clone());

        pep_name
    }
}

/// Converts `uv_normalize::PackageName` to our normalise
pub fn to_normalize(normalise: &uv_normalize::PackageName) -> pep508_rs::PackageName {
    pep508_rs::PackageName::from_str(normalise.as_str()).expect("should be the same")
}

/// Converts `uv_normalize::PackageName` to our normalise
pub fn to_uv_normalize(normalise: &pep508_rs::PackageName) -> uv_normalize::PackageName {
    uv_normalize::PackageName::from_str(normalise.to_string().as_str()).expect("should be the same")
}

/// Converts `pep508_rs::ExtraName` to `uv_normalize::ExtraName`
pub fn to_uv_extra_name(extra_name: &pep508_rs::ExtraName) -> uv_normalize::ExtraName {
    uv_normalize::ExtraName::from_str(extra_name.to_string().as_str()).expect("should be the same")
}
/// Converts `uv_normalize::ExtraName` to `pep508_rs::ExtraName`
pub fn to_extra_name(extra_name: &uv_normalize::ExtraName) -> pep508_rs::ExtraName {
    pep508_rs::ExtraName::from_str(extra_name.to_string().as_str()).expect("should be the same")
}
