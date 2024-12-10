use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};
use tracing::trace;
use url::Url;

use crate::url::RepositoryUrl;
// use uv_auth::Credentials;
// use uv_cache_key::RepositoryUrl;

/// Global authentication cache for a uv invocation.
///
/// This is used to share Git credentials within a single process.
pub static GIT_STORE: LazyLock<GitStore> = LazyLock::new(GitStore::default);

/// A store for Git credentials.
#[derive(Debug, Default)]
pub struct GitStore(RwLock<HashMap<RepositoryUrl, Arc<Credentials>>>);

impl GitStore {
    /// Insert [`Credentials`] for the given URL into the store.
    pub fn insert(&self, url: RepositoryUrl, credentials: Credentials) -> Option<Arc<Credentials>> {
        self.0.write().unwrap().insert(url, Arc::new(credentials))
    }

    /// Get the [`Credentials`] for the given URL, if they exist.
    pub fn get(&self, url: &RepositoryUrl) -> Option<Arc<Credentials>> {
        self.0.read().unwrap().get(url).cloned()
    }
}

/// Populate the global authentication store with credentials on a Git URL, if there are any.
///
/// Returns `true` if the store was updated.
pub fn store_credentials_from_url(url: &Url) -> bool {
    if let Some(credentials) = Credentials::from_url(url) {
        trace!("Caching credentials for {url}");
        GIT_STORE.insert(RepositoryUrl::new(url), credentials);
        true
    } else {
        false
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Credentials {
    /// The name of the user for authentication.
    username: Username,
    /// The password to use for authentication.
    password: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash, Default)]
pub(crate) struct Username(Option<String>);

impl Username {
    /// Create a new username.
    ///
    /// Unlike `reqwest`, empty usernames are be encoded as `None` instead of an empty string.
    pub(crate) fn new(value: Option<String>) -> Self {
        // Ensure empty strings are `None`
        if let Some(value) = value {
            if value.is_empty() {
                Self(None)
            } else {
                Self(Some(value))
            }
        } else {
            Self(value)
        }
    }

    pub(crate) fn none() -> Self {
        Self::new(None)
    }

    pub(crate) fn is_none(&self) -> bool {
        self.0.is_none()
    }

    pub(crate) fn is_some(&self) -> bool {
        self.0.is_some()
    }

    pub(crate) fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl From<String> for Username {
    fn from(value: String) -> Self {
        Self::new(Some(value))
    }
}

impl From<Option<String>> for Username {
    fn from(value: Option<String>) -> Self {
        Self::new(value)
    }
}

impl Credentials {
    pub(crate) fn new(username: Option<String>, password: Option<String>) -> Self {
        Self {
            username: Username::new(username),
            password,
        }
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub(crate) fn to_username(&self) -> Username {
        self.username.clone()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.password.is_none() && self.username.is_none()
    }

    /// Apply the credentials to the given URL.
    ///
    /// Any existing credentials will be overridden.
    #[must_use]
    pub fn apply(&self, mut url: Url) -> Url {
        if let Some(username) = self.username() {
            let _ = url.set_username(username);
        }
        if let Some(password) = self.password() {
            let _ = url.set_password(Some(password));
        }
        url
    }

    /// Parse [`Credentials`] from a URL, if any.
    ///
    /// Returns [`None`] if both [`Url::username`] and [`Url::password`] are not populated.
    pub fn from_url(url: &Url) -> Option<Self> {
        if url.username().is_empty() && url.password().is_none() {
            return None;
        }
        Some(Self {
            // Remove percent-encoding from URL credentials
            // See <https://github.com/pypa/pip/blob/06d21db4ff1ab69665c22a88718a4ea9757ca293/src/pip/_internal/utils/misc.py#L497-L499>
            username: if url.username().is_empty() {
                None
            } else {
                Some(
                    urlencoding::decode(url.username())
                        .expect("An encoded username should always decode")
                        .into_owned(),
                )
            }
            .into(),
            password: url.password().map(|password| {
                urlencoding::decode(password)
                    .expect("An encoded password should always decode")
                    .into_owned()
            }),
        })
    }
}
