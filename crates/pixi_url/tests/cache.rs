use pixi_spec::UrlSpec;
use pixi_url::{UrlError, UrlResolver, UrlSource};
use rattler_digest::{Md5, Md5Hash, Sha256, Sha256Hash, parse_digest_from_hex};
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use tempfile::{TempDir, tempdir};
use url::Url;

const HELLO_WORLD_SHA256: &str = "cceb48dc9667384be394e8c19199252e9e0bdaff98272b19f66a854b4631c163";

fn archive_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/url/hello_world.zip")
}

fn archive_sha() -> Sha256Hash {
    parse_digest_from_hex::<Sha256>(HELLO_WORLD_SHA256).unwrap()
}

fn bogus_md5() -> Md5Hash {
    parse_digest_from_hex::<Md5>("ffffffffffffffffffffffffffffffff").unwrap()
}

fn file_url(tempdir: &TempDir, name: &str) -> Url {
    let path = tempdir.path().join(name);
    fs_err::copy(archive_path(), &path).unwrap();
    Url::from_file_path(&path).unwrap()
}

fn cached_checkout(cache_root: &std::path::Path, sha: Sha256Hash) -> std::path::PathBuf {
    let checkout = cache_root.join("checkouts").join(format!("{sha:x}"));
    fs_err::create_dir_all(&checkout).expect("checkout dir");
    fs_err::write(checkout.join("text.txt"), "Hello, World\n").expect("file");
    fs_err::write(checkout.join(".pixi-url-ready"), "ready").unwrap();
    checkout
}

fn hello_world_bytes() -> Vec<u8> {
    fs_err::read(archive_path()).unwrap()
}

fn tokio_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn panic_client() -> LazyClient {
    LazyClient::new(|| -> ClientWithMiddleware {
        panic!("network should not be used in this test")
    })
}

struct TestHttpServer {
    url: Url,
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TestHttpServer {
    fn new(body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let url = Url::parse(&format!("http://{addr}/archive.zip")).unwrap();

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let handle = thread::spawn(move || {
            while !shutdown_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 1024];
                        loop {
                            match stream.read(&mut buf) {
                                Ok(0) => break,
                                Ok(n) => {
                                    if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(_) => break,
                            }
                        }

                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.write_all(&body);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            url,
            addr,
            shutdown,
            handle: Some(handle),
        }
    }

    fn url(&self) -> &Url {
        &self.url
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn url_source_uses_existing_checkout_when_sha_and_files_present() {
    let rt = tokio_runtime();
    rt.block_on(async {
        let cache = tempdir().unwrap();
        let sha = archive_sha();
        let checkout_dir = cached_checkout(cache.path(), sha);

        let spec = UrlSpec {
            url: Url::parse("https://example.com/hello.zip").unwrap(),
            md5: None,
            sha256: Some(sha),
        };

        let fetch = UrlSource::new(spec, panic_client(), cache.path())
            .fetch()
            .await
            .expect("fetch");

        assert_eq!(fetch.path(), checkout_dir.as_path());
        assert_eq!(fetch.pinned().sha256, sha);
    });
}

#[test]
fn resolver_reuses_cached_sha_without_downloading() {
    let rt = tokio_runtime();
    rt.block_on(async {
        let cache = tempdir().unwrap();
        let sha = archive_sha();
        let checkout_dir = cached_checkout(cache.path(), sha);

        let url = Url::parse("https://example.com/hello.zip").unwrap();
        let resolver = UrlResolver::default();
        resolver.insert(url.clone(), sha);

        let spec = UrlSpec {
            url,
            md5: None,
            sha256: None,
        };

        let fetch = resolver
            .fetch(spec, panic_client(), cache.path().into(), None)
            .await
            .expect("resolver fetch");

        assert_eq!(fetch.path(), checkout_dir.as_path());
        assert_eq!(fetch.pinned().sha256, sha);
    });
}

#[test]
fn url_source_downloads_and_reuses_checkout() {
    let rt = tokio_runtime();
    rt.block_on(async {
        let cache = tempdir().unwrap();
        let archive = tempdir().unwrap();
        let url = file_url(&archive, "hello.zip");
        let client = LazyClient::default();

        let spec = UrlSpec {
            url: url.clone(),
            md5: None,
            sha256: None,
        };

        let first = UrlSource::new(spec.clone(), client.clone(), cache.path())
            .fetch()
            .await
            .expect("download succeeds");
        assert!(first.path().join("text.txt").exists());
        let sha = first.pinned().sha256;

        let second = UrlSource::new(spec, client, cache.path())
            .fetch()
            .await
            .expect("cached fetch succeeds");
        assert_eq!(second.pinned().sha256, sha);
        assert_eq!(second.path(), first.path());
    });
}

#[test]
fn url_source_errors_on_sha_mismatch() {
    let rt = tokio_runtime();
    rt.block_on(async {
        let cache = tempdir().unwrap();
        let archive = tempdir().unwrap();

        let spec = UrlSpec {
            url: file_url(&archive, "sha-mismatch.zip"),
            md5: None,
            sha256: Some(Sha256Hash::from([0u8; 32])),
        };

        let err = UrlSource::new(spec, LazyClient::default(), cache.path())
            .fetch()
            .await
            .expect_err("sha mismatch");
        assert!(matches!(err, UrlError::Sha256Mismatch { .. }));
    });
}

#[test]
fn url_source_errors_on_md5_mismatch() {
    let rt = tokio_runtime();
    rt.block_on(async {
        let cache = tempdir().unwrap();
        let archive = tempdir().unwrap();

        let spec = UrlSpec {
            url: file_url(&archive, "md5-mismatch.zip"),
            md5: Some(bogus_md5()),
            sha256: Some(archive_sha()),
        };

        let err = UrlSource::new(spec, LazyClient::default(), cache.path())
            .fetch()
            .await
            .expect_err("md5 mismatch");
        assert!(matches!(err, UrlError::Md5Mismatch { .. }));
    });
}

#[test]
fn url_source_downloads_over_http_and_extracts_contents() {
    let rt = tokio_runtime();
    rt.block_on(async {
        let server = TestHttpServer::new(hello_world_bytes());
        let cache = tempdir().unwrap();
        let spec = UrlSpec {
            url: server.url().clone(),
            md5: None,
            sha256: Some(archive_sha()),
        };

        let fetch = UrlSource::new(spec, LazyClient::default(), cache.path())
            .fetch()
            .await
            .expect("http download succeeds");

        let text = fs_err::read_to_string(fetch.path().join("text.txt")).unwrap();
        assert!(text.contains("Hello, World"));
    });
}
