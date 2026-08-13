//! End-to-end checks that the certificates pixi loads decide what it trusts.
//!
//! Each test starts a throwaway TLS server presenting a generated certificate,
//! points `SSL_CERT_FILE` or `SSL_CERT_DIR` at some combination of authorities,
//! and asks whether pixi's client is willing to talk to it.

#![cfg(feature = "rustls")]

use std::{
    ffi::OsStr,
    path::Path,
    sync::{Arc, Once},
};

use rcgen::{BasicConstraints, CertificateParams, CustomExtension, IsCa, Issuer, KeyPair};
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer},
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;

/// A generated certificate authority, plus a leaf it has signed.
struct Authority {
    pem: String,
    leaf: CertificateDer<'static>,
    leaf_key: PrivateKeyDer<'static>,
}

impl Authority {
    /// A perfectly ordinary authority.
    fn usable() -> Self {
        Self::new(Vec::new())
    }

    /// An authority carrying `basicConstraints` twice.
    ///
    /// It is well-formed DER and parses as a certificate, but a trust anchor is
    /// only allowed to carry each extension once, so it is refused. This is the
    /// shape that used to take down the whole client.
    fn unusable() -> Self {
        Self::new(vec![CustomExtension::from_oid_content(
            &[2, 5, 29, 19],
            vec![0x30, 0x00],
        )])
    }

    fn new(custom_extensions: Vec<CustomExtension>) -> Self {
        let ca_key = KeyPair::generate().expect("key pair");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("parameters");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.custom_extensions = custom_extensions;
        let pem = ca_params.self_signed(&ca_key).expect("authority").pem();
        let issuer = Issuer::new(ca_params, ca_key);

        let leaf_key = KeyPair::generate().expect("key pair");
        let leaf = CertificateParams::new(vec!["localhost".to_owned()])
            .expect("parameters")
            .signed_by(&leaf_key, &issuer)
            .expect("leaf certificate");

        Self {
            pem,
            leaf: leaf.der().clone(),
            leaf_key: PrivateKeyDer::try_from(leaf_key.serialize_der()).expect("private key"),
        }
    }

    fn write_pem(&self, path: &Path) {
        fs_err::write(path, &self.pem).expect("write certificate");
    }
}
/// Pick a crypto provider for the test server.
///
/// Feature unification pulls in more than one provider on some platforms, and
/// rustls refuses to guess between them, so say which one to use.
fn install_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Serve HTTPS on a loopback port until the test ends.
async fn serve(authority: &Authority) -> u16 {
    install_crypto_provider();
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![authority.leaf.clone()], authority.leaf_key.clone_key())
        .expect("server config");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let port = listener.local_addr().expect("local address").port();

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    let _ = tls.read(&mut [0u8; 1024]).await;
                    let _ = tls
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                        .await;
                    let _ = tls.shutdown().await;
                }
            });
        }
    });

    port
}

/// Build a client the way pixi builds its own, with the given environment.
fn client(ssl_cert_file: Option<&Path>, ssl_cert_dir: Option<&OsStr>) -> reqwest::Client {
    temp_env::with_vars(
        [
            ("SSL_CERT_FILE", ssl_cert_file.map(Path::as_os_str)),
            ("SSL_CERT_DIR", ssl_cert_dir),
        ],
        || {
            pixi_utils::reqwest::reqwest_client_builder(None)
                .expect("client builder")
                .build()
                .expect("client")
        },
    )
}

async fn can_reach(client: &reqwest::Client, port: u16) -> bool {
    match client
        .get(format!("https://localhost:{port}/"))
        .send()
        .await
    {
        Ok(_) => true,
        Err(err) => {
            eprintln!("request failed: {err:?}");
            false
        }
    }
}

#[tokio::test]
async fn a_bundle_keeps_working_when_one_certificate_is_unusable() {
    let server = Authority::usable();
    let port = serve(&server).await;
    let dir = TempDir::new().expect("temp dir");
    let bundle = dir.path().join("bundle.pem");
    fs_err::write(
        &bundle,
        format!("{}{}", Authority::unusable().pem, server.pem),
    )
    .expect("bundle");

    assert!(can_reach(&client(Some(&bundle), None), port).await);
}

#[tokio::test]
async fn a_directory_keeps_working_when_one_certificate_is_unusable() {
    let server = Authority::usable();
    let port = serve(&server).await;
    let dir = TempDir::new().expect("temp dir");
    server.write_pem(&dir.path().join("good.pem"));
    Authority::unusable().write_pem(&dir.path().join("bad.pem"));

    assert!(can_reach(&client(None, Some(dir.path().as_os_str())), port).await);
}

#[tokio::test]
async fn nothing_is_trusted_when_every_certificate_is_unusable() {
    let server = Authority::usable();
    let port = serve(&server).await;
    let dir = TempDir::new().expect("temp dir");
    let bundle = dir.path().join("bundle.pem");
    Authority::unusable().write_pem(&bundle);

    assert!(!can_reach(&client(Some(&bundle), None), port).await);
}

#[tokio::test]
async fn a_file_and_a_directory_are_combined() {
    let server = Authority::usable();
    let port = serve(&server).await;
    let file_dir = TempDir::new().expect("temp dir");
    let bundle = file_dir.path().join("other.pem");
    Authority::usable().write_pem(&bundle);
    let cert_dir = TempDir::new().expect("temp dir");
    server.write_pem(&cert_dir.path().join("good.pem"));

    let client = client(Some(&bundle), Some(cert_dir.path().as_os_str()));
    assert!(can_reach(&client, port).await);
}

#[tokio::test]
async fn missing_directory_entries_are_skipped() {
    let server = Authority::usable();
    let port = serve(&server).await;
    let dir = TempDir::new().expect("temp dir");
    server.write_pem(&dir.path().join("good.pem"));
    let entries =
        std::env::join_paths([dir.path().join("missing"), dir.path().to_path_buf()]).expect("dirs");

    assert!(can_reach(&client(None, Some(entries.as_os_str())), port).await);
}
