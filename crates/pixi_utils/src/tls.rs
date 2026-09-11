//! TLS certificate loading for pixi's reqwest client.
//!
//! [`Certificates`] is a thin newtype over [`CertificateDer<'static>`], with
//! factories for the bundled webpki roots, the platform's native store, and the
//! `SSL_CERT_FILE` / `SSL_CERT_DIR` environment variables.

use std::{env, io, path::PathBuf};

use itertools::Itertools;
use pixi_config::TlsRootCerts;
use rustls_native_certs::{CertificateResult, load_certs_from_paths};
use rustls_pki_types::CertificateDer;
#[cfg(feature = "rustls")]
use webpki::{Error as WebpkiError, anchor_from_trusted_cert};
#[cfg(feature = "rustls")]
use x509_parser::prelude::{FromDer, X509Certificate};

/// Where a certificate came from.
///
/// Only used for diagnostics: when pixi drops a certificate this says which
/// file or store to go look in.
#[derive(Debug, Clone)]
enum CertificateSource {
    NativeStore,
    SslCertFile(PathBuf),
    SslCertDir(PathBuf),
}

impl std::fmt::Display for CertificateSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NativeStore => write!(f, "the system trust store"),
            Self::SslCertFile(path) => write!(f, "`SSL_CERT_FILE` ({})", path.display()),
            Self::SslCertDir(path) => write!(f, "`SSL_CERT_DIR` ({})", path.display()),
        }
    }
}

/// Explain why rustls refused a certificate as a trust anchor.
///
/// The raw errors are terse and say nothing about which certificate failed, so
/// pull the subject out of the DER.
///
/// Only the variants that `anchor_from_trusted_cert` can actually return are
/// named. It parses anchors with an ignore-unknown-critical-extension policy,
/// and it rewrites a v1 version field to `BadDer` itself, so the extension and
/// version errors never reach us.
#[cfg(feature = "rustls")]
fn describe_rejection(cert: &CertificateDer<'_>, err: WebpkiError) -> String {
    let reason = match err {
        WebpkiError::BadDer => "is malformed DER",
        WebpkiError::TrailingData(_) => "has trailing data",
        WebpkiError::ExtensionValueInvalid => "has a duplicate or invalid extension",
        // The algorithm named inside the certificate has to match the one on the
        // outside byte for byte. Certificates that differ only in the optional
        // RSA NULL parameter land here, which is a common way for a hand-rolled
        // corporate CA to be unusable.
        WebpkiError::SignatureAlgorithmMismatch => {
            "names a different signature algorithm inside than it is signed with"
        }
        _ => "cannot be used as a trust anchor",
    };

    // Parsing can fail where webpki already refused the DER. The reason on its
    // own is still worth logging.
    let Ok((_, parsed)) = X509Certificate::from_der(cert.as_ref()) else {
        return reason.to_owned();
    };

    let subject = parsed.subject();
    if subject.iter_attributes().next().is_some() {
        format!("`{subject}` {reason}")
    } else {
        reason.to_owned()
    }
}
/// A collection of TLS certificates in DER form.
#[derive(Debug, Clone, Default)]
pub struct Certificates(Vec<CertificateDer<'static>>);

impl Certificates {
    /// Resolve the certificates to install on pixi's reqwest client.
    ///
    /// Priority:
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
        let loaded = result.certs.len();
        let certs = Self::from(result).filter_invalid(&CertificateSource::NativeStore);
        if certs.0.is_empty() {
            // An empty anchor set is accepted by rustls without complaint, so
            // every request would fail later with an unrelated-looking issuer
            // error. Say it here, where the cause is still visible.
            tracing::warn!(
                "no usable certificates in the system trust store ({loaded} loaded); TLS connections will fail. Set `tls-root-certs = \"webpki\"` or point `SSL_CERT_FILE` at a bundle."
            );
        }
        certs
    }

    /// Load custom CA certificates from `SSL_CERT_FILE` and `SSL_CERT_DIR`.
    ///
    /// Returns `None` only when neither variable is set to a non-empty value.
    /// Setting either one is taken as a deliberate choice about what to trust,
    /// so it replaces the default roots even when it turns out to be missing,
    /// unreadable, or empty of usable certificates. Falling back would quietly
    /// widen trust to the public web PKI, which is the opposite of what someone
    /// narrowing trust to their own CA asked for.
    pub fn from_env() -> Option<Self> {
        let mut certs = Self::default();
        let mut has_source = false;

        if let Some(ssl_cert_file) = env::var_os("SSL_CERT_FILE")
            && !ssl_cert_file.is_empty()
        {
            has_source = true;
            if let Some(file_certs) = Self::from_ssl_cert_file(&ssl_cert_file) {
                certs.merge(file_certs);
            }
        }

        if let Some(ssl_cert_dir) = env::var_os("SSL_CERT_DIR")
            && !ssl_cert_dir.is_empty()
        {
            has_source = true;
            if let Some(dir_certs) = Self::from_ssl_cert_dir(&ssl_cert_dir) {
                certs.merge(dir_certs);
            }
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
                let certs = Self::from(result)
                    .filter_invalid(&CertificateSource::SslCertFile(file.clone()));
                if certs.0.is_empty() {
                    tracing::warn!(
                        "no usable certificates in `SSL_CERT_FILE`: {}",
                        file.display()
                    );
                    return None;
                }
                Some(certs)
            }
            Ok(_) => {
                tracing::warn!(
                    "invalid `SSL_CERT_FILE`: path is not a file: {}",
                    file.display()
                );
                None
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                tracing::warn!(
                    "invalid `SSL_CERT_FILE`: path does not exist: {}",
                    file.display()
                );
                None
            }
            Err(err) => {
                tracing::warn!("invalid `SSL_CERT_FILE` ({}): {err}", file.display());
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
                "invalid `SSL_CERT_DIR`: none of {} exist",
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
        let mut loaded = 0;
        for dir in &existing {
            let result = load_certs_from_paths(None, Some(dir.as_path()));
            for err in &result.errors {
                tracing::warn!("failed to load `SSL_CERT_DIR` ({}): {err}", dir.display());
            }
            let dir_certs = Self::from(result);
            loaded += dir_certs.0.len();
            certs.merge(dir_certs.filter_invalid(&CertificateSource::SslCertDir(dir.clone())));
        }

        if certs.0.is_empty() {
            let dirs = existing.iter().map(|p| p.display().to_string()).join(", ");
            if loaded == 0 {
                // An existing but empty directory is normal, not a misconfiguration.
                // conda-forge's `openssl` ships `$PREFIX/ssl/certs` with nothing but a
                // `.keep` placeholder and its activation script points `SSL_CERT_DIR`
                // at it, while the real bundle sits next to it in `SSL_CERT_FILE`.
                // Warning would fire on every command and there is nothing to fix.
                tracing::debug!("no certificates in `SSL_CERT_DIR`: {dirs}");
            } else {
                // The directory did hold certificates and not one survived. That
                // is a real misconfiguration, so keep it at a level the user sees.
                tracing::warn!(
                    "none of the {loaded} certificates in `SSL_CERT_DIR` ({dirs}) can be used; run with `-vvv` to see why each was rejected"
                );
            }
            return None;
        }
        Some(certs)
    }

    /// Drop certificates that rustls refuses as trust anchors.
    ///
    /// reqwest does not check the DER until the client is built, and then it
    /// fails the whole build with one opaque error. A single bad certificate in
    /// a corporate bundle therefore takes down every request pixi makes.
    /// Dropping it keeps the rest of the bundle working. If a server genuinely
    /// needs the dropped certificate, that request fails on its own and names
    /// the server.
    ///
    /// Logged at debug: the certificate is almost never the user's to fix, and
    /// the trust store on a managed machine routinely holds a few entries
    /// rustls will not parse.
    #[cfg(feature = "rustls")]
    fn filter_invalid(mut self, source: &CertificateSource) -> Self {
        self.0.retain(|cert| {
            if let Err(err) = anchor_from_trusted_cert(cert) {
                tracing::debug!(
                    "ignoring certificate from {source}: {}",
                    describe_rejection(cert, err)
                );
                return false;
            }
            true
        });
        self
    }

    /// Keep every certificate on native-tls builds.
    ///
    /// The platform verifier decides what it trusts, and it accepts anchors
    /// rustls rejects. Filtering here would drop certificates that work.
    #[cfg(not(feature = "rustls"))]
    fn filter_invalid(self, _source: &CertificateSource) -> Self {
        self
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
    use super::*;

    /// Rewrite a real root certificate's explicit version field from v3 to v1.
    ///
    /// The DER keeps its length and still parses as a certificate, but
    /// `anchor_from_trusted_cert` refuses it. That is exactly the shape of
    /// certificate that used to fail the whole `ClientBuilder::build()`.
    #[cfg(feature = "rustls")]
    fn unusable_cert(der: &[u8]) -> CertificateDer<'static> {
        const EXPLICIT_V3: [u8; 5] = [0xA0, 0x03, 0x02, 0x01, 0x02];
        let mut out = der.to_vec();
        let pos = out
            .windows(EXPLICIT_V3.len())
            .take(40)
            .position(|window| window == EXPLICIT_V3)
            .expect("explicit version field near the start of the TBSCertificate");
        out[pos + 4] = 0x00;
        CertificateDer::from(out)
    }

    #[cfg(feature = "rustls")]
    #[test]
    fn filter_invalid_drops_only_the_unusable_certificate() {
        let valid: Vec<CertificateDer<'static>> = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .take(3)
            .map(|cert| cert.clone().into_owned())
            .collect();
        let mut input = valid.clone();
        input.push(unusable_cert(
            webpki_root_certs::TLS_SERVER_ROOT_CERTS[5].as_ref(),
        ));

        // Unfiltered, the single bad certificate takes down the whole client.
        assert!(
            reqwest::Client::builder()
                .use_rustls_tls()
                .tls_certs_only(Certificates(input.clone()).to_reqwest_certs())
                .build()
                .is_err(),
            "expected the unusable certificate to break the client build"
        );

        let filtered = Certificates(input)
            .filter_invalid(&CertificateSource::SslCertFile(PathBuf::from("bundle.pem")));

        assert_eq!(
            filtered.0, valid,
            "only the unusable certificate should be dropped"
        );
        assert!(
            reqwest::Client::builder()
                .use_rustls_tls()
                .tls_certs_only(filtered.to_reqwest_certs())
                .build()
                .is_ok(),
            "the client should build once the unusable certificate is dropped"
        );
    }

    #[test]
    fn ssl_cert_file_empty_value_is_not_a_source() {
        assert!(Certificates::from_ssl_cert_file(std::ffi::OsStr::new("")).is_none());
    }

    #[test]
    fn ssl_cert_file_nonexistent_path_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.pem");
        assert!(Certificates::from_ssl_cert_file(missing.as_os_str()).is_none());
    }

    #[test]
    fn ssl_cert_file_without_certificates_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.pem");
        fs_err::write(&path, "not a certificate").unwrap();
        assert!(Certificates::from_ssl_cert_file(path.as_os_str()).is_none());
    }

    #[test]
    fn ssl_cert_dir_empty_value_is_not_a_source() {
        assert!(Certificates::from_ssl_cert_dir(std::ffi::OsStr::new("")).is_none());
    }

    #[test]
    fn ssl_cert_dir_nonexistent_path_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = env::join_paths([dir.path().join("missing")]).unwrap();
        assert!(Certificates::from_ssl_cert_dir(missing.as_os_str()).is_none());
    }

    /// The case that made pixi warn on every command: the directory exists and
    /// holds no certificates, which is how the `openssl` package ships it.
    #[test]
    fn ssl_cert_dir_that_exists_but_is_empty_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join(".keep"), "").unwrap();
        let dirs = env::join_paths([dir.path()]).unwrap();
        assert!(Certificates::from_ssl_cert_dir(dirs.as_os_str()).is_none());
    }

    /// A configured source is a deliberate choice about what to trust, so it
    /// must not silently widen back out to the default roots when it turns out
    /// to be unusable.
    #[test]
    fn explicit_cert_file_is_authoritative_even_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.pem");

        temp_env::with_vars(
            [
                ("SSL_CERT_FILE", Some(missing.as_os_str())),
                ("SSL_CERT_DIR", None),
            ],
            || {
                let certs = Certificates::from_env()
                    .expect("a configured `SSL_CERT_FILE` must not fall back to the defaults");
                assert!(
                    certs.is_empty(),
                    "nothing was loadable, so nothing is trusted"
                );
            },
        );
    }

    #[test]
    fn explicit_cert_dir_is_authoritative_even_when_empty() {
        let dir = tempfile::tempdir().unwrap();

        temp_env::with_vars(
            [
                ("SSL_CERT_FILE", None),
                ("SSL_CERT_DIR", Some(dir.path().as_os_str())),
            ],
            || {
                let certs = Certificates::from_env()
                    .expect("a configured `SSL_CERT_DIR` must not fall back to the defaults");
                assert!(certs.is_empty());
            },
        );
    }

    #[test]
    fn unset_cert_vars_leave_the_configured_mode_alone() {
        temp_env::with_vars(
            [
                ("SSL_CERT_FILE", None::<&std::ffi::OsStr>),
                ("SSL_CERT_DIR", None),
            ],
            || assert!(Certificates::from_env().is_none()),
        );
    }

    #[test]
    fn merge_deduplicates() {
        let one = Certificates(
            webpki_root_certs::TLS_SERVER_ROOT_CERTS
                .iter()
                .take(2)
                .map(|cert| cert.clone().into_owned())
                .collect(),
        );
        let mut merged = one.clone();
        merged.merge(one);
        assert_eq!(
            merged.0.len(),
            2,
            "merging a set with itself changes nothing"
        );
    }

    #[test]
    fn webpki_roots_are_not_empty() {
        assert!(!Certificates::webpki_roots().is_empty());
    }
}
