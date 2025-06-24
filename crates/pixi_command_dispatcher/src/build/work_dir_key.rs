//! See [`WorkDirKey`] for more information.

use std::{
    ffi::OsStr,
    hash::{Hash, Hasher},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_record::{PinnedSourceSpec, SourceRecord};
use rattler_conda_types::Platform;
use xxhash_rust::xxh3::Xxh3;

use crate::SourceCheckout;

#[derive(derive_more::From)]
pub enum SourceRecordOrCheckout {
    /// A source record that has not been checked out yet.
    Record(Box<SourceRecord>),

    /// A source checkout that has already been checked out.
    Checkout(Box<SourceCheckout>),
}

impl SourceRecordOrCheckout {
    pub fn pinned(&self) -> &PinnedSourceSpec {
        match self {
            SourceRecordOrCheckout::Record(record) => &record.source,
            SourceRecordOrCheckout::Checkout(checkout) => &checkout.pinned,
        }
    }
}

/// A key to uniquely identify a work directory. If there is a source build with
/// the same key, they will share the same working directory.
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
            SourceRecordOrCheckout::Record(record) => {
                Some(record.package_record.name.as_normalized())
            }
            SourceRecordOrCheckout::Checkout(checkout) => {
                checkout.path.file_name().and_then(OsStr::to_str)
            }
        };

        match name {
            Some(name) => format!("{name}-{unique_key}"),
            None => unique_key,
        }
    }
}
