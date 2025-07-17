//! See [`WorkDirKey`] for more information.

use std::{
    ffi::OsStr,
    hash::{Hash, Hasher},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_record::{PinnedSourceSpec};
use rattler_conda_types::{PackageName, Platform};
use xxhash_rust::xxh3::Xxh3;

use crate::SourceCheckout;

#[derive(derive_more::From)]
pub enum SourceRecordOrCheckout {
    /// A source record that has not been checked out yet.
    Record {
        pinned: PinnedSourceSpec,
        package_name: PackageName,
    },

    /// A source checkout that has already been checked out.
    Checkout { checkout: SourceCheckout },
}

impl SourceRecordOrCheckout {
    pub fn pinned(&self) -> &PinnedSourceSpec {
        match self {
            SourceRecordOrCheckout::Record { pinned, .. } => pinned,
            SourceRecordOrCheckout::Checkout { checkout } => &checkout.pinned,
        }
    }
}

/// A key to uniquely identify a work directory. If there is a source build with
/// the same key, they will share the same working directory.
///
/// TODO: Should we use references instead of owned types here?
pub struct WorkDirKey {
    /// The location of the source
    pub source: SourceRecordOrCheckout,

    /// The platform the dependency will run on
    pub host_platform: Platform,

    /// The build backend name
    /// TODO: Maybe we should also include the version?
    pub build_backend: String,
}

impl WorkDirKey {
    pub fn key(&self) -> String {
        let mut hasher = Xxh3::new();
        self.source.pinned().to_string().hash(&mut hasher);
        self.host_platform.to_string().hash(&mut hasher);
        self.build_backend.hash(&mut hasher);
        let unique_key = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());

        let name = match &self.source {
            SourceRecordOrCheckout::Record { package_name, .. } => {
                Some(package_name.as_normalized())
            }
            SourceRecordOrCheckout::Checkout { checkout} => {
                checkout.path.file_name().and_then(OsStr::to_str)
            }
        };

        match name {
            Some(name) => format!("{name}-{unique_key}"),
            None => unique_key,
        }
    }
}
