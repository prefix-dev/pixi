//! TLS certificate loading for pixi's reqwest client.
//!
//! Mirrors the [`Certificates`] design used by `uv-client` (uv PR #18550): a thin
//! newtype over [`CertificateDer<'static>`] with factories for the bundled
//! webpki roots, the platform's native store, and the `SSL_CERT_FILE` /
//! `SSL_CERT_DIR` environment variables.

use std::{env, io, path::PathBuf};

use itertools::Itertools;
use pixi_config::TlsRootCerts;
use rustls_native_certs::{CertificateResult, load_certs_from_paths};
use rustls_pki_types::CertificateDer;

/// A collection of TLS certificates in DER form.
#[derive(Debug, Clone, Default)]
pub struct Certificates(Vec<CertificateDer<'static>>);

impl Certificates {
    /// Resolve the certificates to install on pixi's reqwest client.
    ///
    /// Priority follows uv's model:
    /// 1. `SSL_CERT_FILE` / `SSL_CERT_DIR` env vars (if set and valid)
    /// 2. The configured [`TlsRootCerts`] mode
    ///
    /// Deprecation warnings for the legacy [`TlsRootCerts::LegacyNative`] and
    /// [`TlsRootCerts::All`] spellings fire once at config-load time
    /// (`Config::from_toml`), so this function stays silent.
    pub fn for_mode(mode: TlsRootCerts) -> Self {
        if let Some(env_certs) = Self::from_env() {
            return env_certs;
        }

        #[allow(deprecated)]
        match mode {
            TlsRootCerts::Webpki => Self::webpki_roots(),
            TlsRootCerts::System | TlsRootCerts::LegacyNative | TlsRootCerts::All => {
                Self::from_native_store()
            }
        }
    }

    /// Load the bundled Mozilla root certificates from `webpki-root-certs`.
    pub fn webpki_roots() -> Self {
        // Each `CertificateDer` borrows from static data, so cloning the slice
        // only copies fat pointers, not certificate bytes.
        Self(webpki_root_certs::TLS_SERVER_ROOT_CERTS.to_vec())
    }

    /// Load certificates from the platform's native trust store via
    /// [`rustls_native_certs::load_native_certs`].
    pub fn from_native_store() -> Self {
        let result = rustls_native_certs::load_native_certs();
        for err in &result.errors {
            tracing::warn!("failed to load a native root certificate: {err}");
        }
        Self::from(result)
    }

    /// Load custom CA certificates from `SSL_CERT_FILE` and `SSL_CERT_DIR`.
    ///
    /// Returns `None` if neither variable is set, the referenced paths are
    /// missing or inaccessible, or no valid certificates are found (with a
    /// warning emitted in each case).
    pub fn from_env() -> Option<Self> {
        let mut certs = Self::default();
        let mut has_source = false;

        if let Some(ssl_cert_file) = env::var_os("SSL_CERT_FILE")
            && let Some(file_certs) = Self::from_ssl_cert_file(&ssl_cert_file)
        {
            has_source = true;
            certs.merge(file_certs);
        }

        if let Some(ssl_cert_dir) = env::var_os("SSL_CERT_DIR")
            && let Some(dir_certs) = Self::from_ssl_cert_dir(&ssl_cert_dir)
        {
            has_source = true;
            certs.merge(dir_certs);
        }

        if has_source { Some(certs) } else { None }
    }

    fn from_ssl_cert_file(value: &std::ffi::OsStr) -> Option<Self> {
        if value.is_empty() {
            return None;
        }
        let file = PathBuf::from(value);
        match file.metadata() {
            Ok(metadata) if metadata.is_file() => {
                let result = load_certs_from_paths(Some(&file), None);
                for err in &result.errors {
                    tracing::warn!("failed to load `SSL_CERT_FILE` ({}): {err}", file.display());
                }
                let certs = Self::from(result);
                if certs.0.is_empty() {
                    tracing::warn!(
                        "ignoring `SSL_CERT_FILE`: no certificates found in {}",
                        file.display()
                    );
                    return None;
                }
                Some(certs)
            }
            Ok(_) => {
                tracing::warn!(
                    "ignoring invalid `SSL_CERT_FILE`: path is not a file: {}",
                    file.display()
                );
                None
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                tracing::warn!(
                    "ignoring invalid `SSL_CERT_FILE`: path does not exist: {}",
                    file.display()
                );
                None
            }
            Err(err) => {
                tracing::warn!(
                    "ignoring invalid `SSL_CERT_FILE` ({}): {err}",
                    file.display()
                );
                None
            }
        }
    }

    fn from_ssl_cert_dir(value: &std::ffi::OsStr) -> Option<Self> {
        if value.is_empty() {
            return None;
        }

        let (existing, missing): (Vec<_>, Vec<_>) =
            env::split_paths(value).partition(|p| p.exists());

        if existing.is_empty() {
            tracing::warn!(
                "ignoring invalid `SSL_CERT_DIR`: none of {} exist",
                missing.iter().map(|p| p.display().to_string()).join(", ")
            );
            return None;
        }
        if !missing.is_empty() {
            tracing::warn!(
                "skipping non-existent entries in `SSL_CERT_DIR`: {}",
                missing.iter().map(|p| p.display().to_string()).join(", ")
            );
        }

        let mut certs = Self::default();
        for dir in &existing {
            let result = load_certs_from_paths(None, Some(dir.as_path()));
            for err in &result.errors {
                tracing::warn!("failed to load `SSL_CERT_DIR` ({}): {err}", dir.display());
            }
            certs.merge(Self::from(result));
        }

        if certs.0.is_empty() {
            tracing::warn!(
                "ignoring `SSL_CERT_DIR`: no certificates found in {}",
                existing.iter().map(|p| p.display().to_string()).join(", ")
            );
            return None;
        }
        Some(certs)
    }

    /// Whether this collection is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Merge another set of certificates into this one, deduplicating after.
    pub fn merge(&mut self, other: Self) {
        self.0.extend(other.0);
        self.0.sort_unstable_by(|a, b| a.as_ref().cmp(b.as_ref()));
        self.0.dedup();
    }

    /// Convert to `reqwest::Certificate` values for use with
    /// [`reqwest::ClientBuilder::tls_certs_only`].
    pub fn to_reqwest_certs(&self) -> Vec<reqwest::Certificate> {
        self.0
            .iter()
            .filter_map(|cert| reqwest::Certificate::from_der(cert.as_ref()).ok())
            .collect()
    }
}

impl From<CertificateResult> for Certificates {
    fn from(result: CertificateResult) -> Self {
        Self(result.certs)
    }
}
