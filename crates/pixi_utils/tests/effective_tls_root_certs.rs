//! Tests for [`effective_tls_root_certs`].
//!
//! These mutate `SSL_CERT_FILE` / `SSL_CERT_DIR`, so they live in their own
//! test binary: the unit tests build HTTP clients that read the same variables
//! concurrently.

use base64::{Engine, engine::general_purpose::STANDARD};
use pixi_config::TlsRootCerts;
use pixi_utils::{
    reqwest::{default_tls_root_certs, effective_tls_root_certs},
    tls::Certificates,
};

const NO_CERT_ENV: [(&str, Option<&str>); 2] = [("SSL_CERT_FILE", None), ("SSL_CERT_DIR", None)];

#[test]
fn unset_mode_uses_the_backend_default() {
    temp_env::with_vars(NO_CERT_ENV, || {
        let effective = effective_tls_root_certs(None, Certificates::env_sources());
        assert_eq!(effective.mode, default_tls_root_certs());
        assert!(effective.is_default);
        assert!(effective.env_sources.is_empty());
    });
}

#[test]
#[allow(deprecated)]
fn deprecated_modes_are_reported_as_system() {
    temp_env::with_vars(NO_CERT_ENV, || {
        for mode in [TlsRootCerts::LegacyNative, TlsRootCerts::All] {
            let effective = effective_tls_root_certs(Some(mode), Certificates::env_sources());
            assert_eq!(effective.mode, TlsRootCerts::System);
            assert!(!effective.is_default);
        }
    });
}

#[test]
fn valid_ssl_cert_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ca.pem");
    let der = webpki_root_certs::TLS_SERVER_ROOT_CERTS[0].as_ref();
    let pem = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        STANDARD.encode(der)
    );
    fs_err::write(&path, pem).unwrap();
    let path = path.to_str().unwrap();

    temp_env::with_vars(
        [("SSL_CERT_FILE", Some(path)), ("SSL_CERT_DIR", None)],
        || {
            let effective =
                effective_tls_root_certs(Some(TlsRootCerts::Webpki), Certificates::env_sources());
            assert_eq!(effective.mode, TlsRootCerts::Webpki);
            assert_eq!(effective.env_sources.len(), 1);
            assert_eq!(effective.env_sources[0].variable, "SSL_CERT_FILE");
            assert_eq!(effective.env_sources[0].value, path);
            assert_eq!(
                effective.env_replaces_mode,
                cfg!(not(feature = "native-tls"))
            );
        },
    );
}

#[test]
fn missing_ssl_cert_file_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.pem");
    temp_env::with_vars(
        [
            ("SSL_CERT_FILE", Some(missing.to_str().unwrap())),
            ("SSL_CERT_DIR", None),
        ],
        || {
            let effective =
                effective_tls_root_certs(Some(TlsRootCerts::System), Certificates::env_sources());
            assert!(effective.env_sources.is_empty());
        },
    );
}
