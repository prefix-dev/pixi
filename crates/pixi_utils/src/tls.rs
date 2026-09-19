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
    /// Root certificates for the configured [`TlsRootCerts`] mode are loaded first
    /// (the system trust store for `System`, or Mozilla roots for `Webpki`), and
    /// any certificates found in `SSL_CERT_FILE` or `SSL_CERT_DIR` are merged
    /// into them.
    ///
    /// Deprecation warnings for the legacy [`TlsRootCerts::LegacyNative`] and
    /// [`TlsRootCerts::All`] spellings fire once at config-load time
    /// (`Config::from_toml`), so this function stays silent.
    pub fn for_mode(mode: TlsRootCerts) -> Self {
        #[allow(deprecated)]
        let mut certs = match mode {
            TlsRootCerts::Webpki => Self::webpki_roots(),
            TlsRootCerts::System | TlsRootCerts::LegacyNative | TlsRootCerts::All => {
                Self::from_native_store()
            }
        };

        if let Some(env_certs) = Self::from_env() {
            certs.merge(env_certs);
        }

        certs
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

    /// Number of certificates in this collection.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether this collection is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the certificates as a slice of DER-encoded certificates.
    pub fn as_slice(&self) -> &[CertificateDer<'static>] {
        &self.0
    }

    /// Check if a certificate is contained in this collection.
    pub fn contains(&self, cert: &CertificateDer<'_>) -> bool {
        self.0.iter().any(|c| c.as_ref() == cert.as_ref())
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
    use std::io::Write;

    use pixi_config::TlsRootCerts;
    use tempfile::NamedTempFile;

    use super::*;

    const TEST_CERT_PEM: &str = concat!(
        "-----BEGIN CERTIFICATE-----\n",
        "MIIDDzCCAfegAwIBAgIUKztfxD+3pXjx6ZJeqviN6VntuxAwDQYJKoZIhvcNAQEL\n",
        "BQAwFzEVMBMGA1UEAwwMVGVzdCBQaXhpIENBMB4XDTI2MDkxOTEwNDQzOVoXDTM2\n",
        "MDkxNjEwNDQzOVowFzEVMBMGA1UEAwwMVGVzdCBQaXhpIENBMIIBIjANBgkqhkiG\n",
        "9w0BAQEFAAOCAQ8AMIIBCgKCAQEAwxrwRnx6QlExqc7IdQErCEdfdb2FNqzx8x1g\n",
        "FMWshdmay+v3Qm0Q4hspuHP51R0kwufjuGGW2e3jTDwk4C162O0OZWrZmNJIIiHD\n",
        "sdZ1i8i2FW3r3UuOh+cKpVpyaFMrT4t0brkW0Zy2ws8A3eh7aIywmeYziCRwpSid\n",
        "5thgyc1XExSnQv9hmEKlbT06MOVdnOypy1V0tSPUlm+4rNaUzfvFObs+c8W0QhFv\n",
        "52nBi4Uj+lUoT+33ixKS5RPFZbO0KbBlaFBaWWab6QrGtpVbjR8MVo4U2H+N3/h0\n",
        "ccLSSnE8fu8/+Dmo1lSCza+rbEFJgCH73eIebPbHuqG+Gr46xwIDAQABo1MwUTAd\n",
        "BgNVHQ4EFgQUagYbkw0yCFYHgz5Y3riPJ+lDbZIwHwYDVR0jBBgwFoAUagYbkw0y\n",
        "CFYHgz5Y3riPJ+lDbZIwDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOC\n",
        "AQEApHTBUKjb1PKoHFHn/JgMkAeRA4/A9iRzD9hItzFk+0j/cwTghpYOvcLB99N/\n",
        "VjwrghEuPh78fMvuYHfDMaau3ZwRWWljLU72fXTsTi7zlhuEhkzzxOi4uF6F6FEv\n",
        "fQfEj6QlDYaj5FbH90B9rGqf4ZAAsZRWEl8Gdky/ICEH2BMVT/d7D0zqcxWoMG25\n",
        "iJFe05DCf2kskCIsMlcOz/oTb/rN4yfxOsTQYaypnzQbltaQ28lUmBFH273Chtq1\n",
        "a+5m443UMUGCEYtEucUpwjffkrYeAaVvT3HR6xXCUsSg0Pow2gwPrT7QRx5LM3kC\n",
        "PQRuveraQhjoc/MjmshkDDjONg==\n",
        "-----END CERTIFICATE-----\n"
    );

    #[test]
    fn test_webpki_roots_non_empty() {
        let certs = Certificates::webpki_roots();
        assert!(!certs.is_empty());
        assert!(certs.len() > 100);
    }

    #[test]
    fn test_for_mode_without_env() {
        temp_env::with_vars(
            [
                ("SSL_CERT_FILE", None::<&str>),
                ("SSL_CERT_DIR", None::<&str>),
            ],
            || {
                let webpki = Certificates::for_mode(TlsRootCerts::Webpki);
                assert_eq!(webpki.len(), Certificates::webpki_roots().len());

                let system = Certificates::for_mode(TlsRootCerts::System);
                assert_eq!(system.len(), Certificates::from_native_store().len());
            },
        );
    }

    #[test]
    fn test_for_mode_merges_ssl_cert_file_with_system() {
        let mut temp_cert = NamedTempFile::new().unwrap();
        temp_cert.write_all(TEST_CERT_PEM.as_bytes()).unwrap();

        temp_env::with_vars(
            [
                ("SSL_CERT_FILE", Some(temp_cert.path().to_str().unwrap())),
                ("SSL_CERT_DIR", None::<&str>),
            ],
            || {
                let env_certs = Certificates::from_env().expect("should parse cert from env");
                assert_eq!(env_certs.len(), 1);
                let test_cert = &env_certs.as_slice()[0];

                let system = Certificates::for_mode(TlsRootCerts::System);
                assert!(system.contains(test_cert));

                let native = Certificates::from_native_store();
                if !native.is_empty() {
                    assert!(system.len() >= native.len());
                }
            },
        );
    }

    #[test]
    fn test_for_mode_merges_ssl_cert_file_with_webpki() {
        let mut temp_cert = NamedTempFile::new().unwrap();
        temp_cert.write_all(TEST_CERT_PEM.as_bytes()).unwrap();

        temp_env::with_vars(
            [
                ("SSL_CERT_FILE", Some(temp_cert.path().to_str().unwrap())),
                ("SSL_CERT_DIR", None::<&str>),
            ],
            || {
                let env_certs = Certificates::from_env().expect("should parse cert from env");
                assert_eq!(env_certs.len(), 1);
                let test_cert = &env_certs.as_slice()[0];

                let webpki = Certificates::for_mode(TlsRootCerts::Webpki);
                assert!(webpki.contains(test_cert));
                assert!(
                    webpki.len() > Certificates::webpki_roots().len() || webpki.contains(test_cert)
                );
            },
        );
    }

    #[test]
    fn test_merge_deduplicates() {
        let mut certs1 = Certificates::webpki_roots();
        let initial_len = certs1.len();
        let certs2 = Certificates::webpki_roots();
        certs1.merge(certs2);
        assert_eq!(certs1.len(), initial_len);
    }
}
