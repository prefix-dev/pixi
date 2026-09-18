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
    /// missing or inaccessible, or no valid certificates are found.
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

    pub(crate) fn from_ssl_cert_file(value: &std::ffi::OsStr) -> Option<Self> {
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

    pub(crate) fn from_ssl_cert_dir(value: &std::ffi::OsStr) -> Option<Self> {
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
            // Unlike `SSL_CERT_FILE`, it is plausible for this to be intentionally set to an
            // empty directory that a user could put certificates in (e.g. OpenSSL's
            // activation script in conda environments).
            tracing::debug!(
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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn test_cert_pem() -> String {
        const CHARSET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let der = &webpki_root_certs::TLS_SERVER_ROOT_CERTS[0];
        let mut b64 = String::new();
        for chunk in der.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            b64.push(CHARSET[(b0 >> 2) as usize] as char);
            b64.push(CHARSET[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
            if chunk.len() > 1 {
                b64.push(CHARSET[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char);
            } else {
                b64.push('=');
            }
            if chunk.len() > 2 {
                b64.push(CHARSET[(b2 & 63) as usize] as char);
            } else {
                b64.push('=');
            }
        }
        format!("-----BEGIN CERTIFICATE-----\n{b64}\n-----END CERTIFICATE-----\n")
    }

    #[test]
    fn test_empty_ssl_cert_dir_returns_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        assert!(Certificates::from_ssl_cert_dir(temp_dir.path().as_os_str()).is_none());
    }

    #[test]
    fn test_ssl_cert_dir_with_non_cert_file_returns_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs_err::write(temp_dir.path().join(".keep"), b"").unwrap();
        assert!(Certificates::from_ssl_cert_dir(temp_dir.path().as_os_str()).is_none());
    }

    #[test]
    fn test_ssl_cert_dir_empty_string_returns_none() {
        assert!(Certificates::from_ssl_cert_dir(OsStr::new("")).is_none());
    }

    #[test]
    fn test_ssl_cert_dir_non_existent_returns_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        let non_existent = temp_dir.path().join("does_not_exist");
        assert!(Certificates::from_ssl_cert_dir(non_existent.as_os_str()).is_none());
    }

    #[test]
    fn test_ssl_cert_dir_with_valid_cert() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs_err::write(temp_dir.path().join("test_cert.crt"), test_cert_pem()).unwrap();
        let certs = Certificates::from_ssl_cert_dir(temp_dir.path().as_os_str());
        assert!(certs.is_some());
        assert!(!certs.unwrap().is_empty());
    }

    #[test]
    fn test_ssl_cert_file_with_valid_cert() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cert_path = temp_dir.path().join("ca.pem");
        fs_err::write(&cert_path, test_cert_pem()).unwrap();
        let certs = Certificates::from_ssl_cert_file(cert_path.as_os_str());
        assert!(certs.is_some());
        assert!(!certs.unwrap().is_empty());
    }

    #[test]
    fn test_ssl_cert_file_non_existent_returns_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cert_path = temp_dir.path().join("non_existent.pem");
        assert!(Certificates::from_ssl_cert_file(cert_path.as_os_str()).is_none());
    }

    #[test]
    fn test_from_env_empty_dir_ignored() {
        let temp_dir = tempfile::tempdir().unwrap();
        temp_env::with_vars(
            [
                ("SSL_CERT_DIR", Some(temp_dir.path().as_os_str())),
                ("SSL_CERT_FILE", None),
            ],
            || {
                assert!(Certificates::from_env().is_none());
            },
        );
    }
}
