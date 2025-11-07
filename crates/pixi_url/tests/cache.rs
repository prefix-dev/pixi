use pixi_spec::UrlSpec;
use pixi_url::{UrlResolver, UrlSource};
use rattler_digest::{Sha256, Sha256Hash, digest::Digest};
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;
use tempfile::tempdir;
use url::Url;

const HELLO_WORLD_ZIP_FILE: &[u8] = &[
    80, 75, 3, 4, 10, 0, 0, 0, 0, 0, 244, 123, 36, 88, 144, 58, 246, 64, 13, 0, 0, 0, 13, 0, 0, 0,
    8, 0, 28, 0, 116, 101, 120, 116, 46, 116, 120, 116, 85, 84, 9, 0, 3, 4, 130, 150, 101, 6, 130,
    150, 101, 117, 120, 11, 0, 1, 4, 245, 1, 0, 0, 4, 20, 0, 0, 0, 72, 101, 108, 108, 111, 44, 32,
    87, 111, 114, 108, 100, 10, 80, 75, 1, 2, 30, 3, 10, 0, 0, 0, 0, 0, 244, 123, 36, 88, 144, 58,
    246, 64, 13, 0, 0, 0, 13, 0, 0, 0, 8, 0, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 164, 129, 0, 0, 0, 0,
    116, 101, 120, 116, 46, 116, 120, 116, 85, 84, 5, 0, 3, 4, 130, 150, 101, 117, 120, 11, 0, 1,
    4, 245, 1, 0, 0, 4, 20, 0, 0, 0, 80, 75, 5, 6, 0, 0, 0, 0, 1, 0, 1, 0, 78, 0, 0, 0, 79, 0, 0,
    0, 0, 0,
];

fn archive_sha() -> Sha256Hash {
    let mut hasher = Sha256::default();
    hasher.update(HELLO_WORLD_ZIP_FILE);
    hasher.finalize()
}

fn cached_checkout(cache_root: &std::path::Path, sha: Sha256Hash) -> std::path::PathBuf {
    let checkout = cache_root.join("checkouts").join(format!("{sha:x}"));
    std::fs::create_dir_all(&checkout).expect("checkout dir");
    std::fs::write(checkout.join("text.txt"), "Hello, World\n").expect("file");
    checkout
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
