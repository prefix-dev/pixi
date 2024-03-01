use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use base64::Engine;
use build_html::{Html, HtmlContainer};
use indoc::formatdoc;
use itertools::Itertools;
use miette::{miette, IntoDiagnostic};
use rattler_networking::Authentication;
use std::collections::BTreeSet;
use std::future::IntoFuture;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tower_http::add_extension::AddExtensionLayer;
use tower_http::auth::AddAuthorizationLayer;
use zip::write::FileOptions;
use zip::ZipWriter;

const WHEEL_TAG: &str = "py3-none-any";
const WHEEL_VERSION: &str = "0.1.0";
const INIT_PY: &str = "__init__.py";

fn make_manifest_file(package_name: &str) -> String {
    formatdoc! {"
        Metadata-Version: 2.3
        Name: {package_name}
        Version: {WHEEL_VERSION}
        Summary: {package_name} is a package
    "}
}

fn make_wheel_file() -> String {
    formatdoc! {"
        Wheel-Version: 1.0
        Generator: pixi_tests (0.0.1)
        Root-Is-Purelib: true
        Tag: {WHEEL_TAG}
    "}
}

fn normalize_package_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

fn record_hash(data: &[u8]) -> String {
    let hash = rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(data).to_vec();
    BASE64_URL_SAFE_NO_PAD.encode(&hash)
}

struct ArchiveFile {
    path_name: String,
    data: Vec<u8>,
}

fn make_record_file(record_path: &str, files: &[&ArchiveFile]) -> String {
    let files = files
        .iter()
        .map(|file| {
            let hash = record_hash(&file.data);
            format!("{},sha256={},{}", file.path_name, hash, file.data.len())
        })
        .join("\n");

    formatdoc! {"
        {files}
        {record_path},,
    "}
}

fn make_wheel(package_name: &str) -> miette::Result<Vec<u8>> {
    let normalized_name = normalize_package_name(package_name);
    let dist_info = format!("{}-{}.dist-info", normalized_name, WHEEL_VERSION);

    let metadata_file = ArchiveFile {
        path_name: format!("{}/METADATA", dist_info),
        data: make_manifest_file(package_name).into_bytes(),
    };

    let top_level_file = ArchiveFile {
        path_name: format!("{}/top_level.txt", dist_info),
        data: normalized_name.clone().into_bytes(),
    };

    let wheel_file = ArchiveFile {
        path_name: format!("{}/WHEEL", dist_info),
        data: make_wheel_file().into_bytes(),
    };

    let init_py_file = ArchiveFile {
        path_name: format!("{}/{}", normalized_name, INIT_PY),
        data: Vec::new(),
    };

    let record_file_path = format!("{}/RECORD", dist_info);
    let record_file = make_record_file(
        &record_file_path,
        &[&metadata_file, &top_level_file, &wheel_file, &init_py_file],
    );

    let record_file = ArchiveFile {
        path_name: record_file_path,
        data: record_file.into_bytes(),
    };

    let files = [
        metadata_file,
        top_level_file,
        wheel_file,
        init_py_file,
        record_file,
    ];

    let mut buffer: Vec<u8> = Vec::new();
    let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));

    let options = FileOptions::default();
    for file in files.into_iter() {
        zip.start_file(file.path_name, options).into_diagnostic()?;
        zip.write_all(&file.data).into_diagnostic()?;
    }

    zip.finish().into_diagnostic()?;
    drop(zip);

    Ok(buffer)
}

type ServedPackages = Arc<BTreeSet<String>>;

async fn get_index(
    axum::Extension(served_packages): axum::Extension<ServedPackages>,
) -> impl IntoResponse {
    let mut document = build_html::HtmlPage::new();

    served_packages.iter().for_each(|package| {
        let href = format!("/{package}");
        document.add_link(href, package);
    });

    axum::response::Html(document.to_html_string())
}

async fn get_package_links(
    axum::Extension(served_packages): axum::Extension<ServedPackages>,
    axum::extract::Path(requested_package): axum::extract::Path<String>,
) -> impl IntoResponse {
    if !served_packages.contains(&requested_package) {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }

    let normalized_name = requested_package.replace('-', "_");
    let wheel_name = format!("{}-{}-{}.whl", normalized_name, WHEEL_VERSION, WHEEL_TAG);

    let mut document = build_html::HtmlPage::new();
    let href = format!("/files/{wheel_name}");
    document.add_link(href, wheel_name);

    axum::response::Html(document.to_html_string()).into_response()
}

async fn get_wheel(
    axum::Extension(served_packages): axum::Extension<ServedPackages>,
    axum::extract::Path(requested_file): axum::extract::Path<String>,
) -> impl IntoResponse {
    let requested_file = match requested_file.strip_suffix(".whl") {
        Some(file) => file,
        None => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };

    let parts = requested_file.split('-').collect::<Vec<_>>();
    let (base_name, version, py_ver, abi, platform) = match parts.as_slice() {
        [base_name, version, py_ver, abi, platform] => {
            (*base_name, *version, *py_ver, *abi, *platform)
        }
        _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };

    let tag = format!("{}-{}-{}", py_ver, abi, platform);
    if version != WHEEL_VERSION || tag != WHEEL_TAG {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }

    let base_name = base_name.to_string();
    if served_packages
        .iter()
        .find(|value| {
            let value = value.replace('-', "_");
            value == base_name
        })
        .is_none()
    {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }

    let wheel = make_wheel(&base_name).unwrap();
    let content_type = "application/zip";

    axum::http::Response::builder()
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, wheel.len())
        .body(axum::body::Body::from(wheel))
        .expect("Failed to build response")
}

pub async fn make_simple_server(
    package_names: &[&str],
    require_auth: Option<Authentication>,
) -> miette::Result<(String, JoinHandle<Result<(), std::io::Error>>)> {
    let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let address = listener.local_addr().into_diagnostic()?;

    let package_names = package_names
        .iter()
        .map(|s| s.to_string())
        .collect::<BTreeSet<_>>();
    let package_names = Arc::new(package_names);

    let router = Router::new()
        .route("/simple", get(get_index))
        .route("/simple/:package/", get(get_package_links))
        .route("/files/:file", get(get_wheel))
        .layer(AddExtensionLayer::new(package_names));

    let router = match require_auth {
        Some(Authentication::BasicHTTP { username, password }) => {
            router.layer(AddAuthorizationLayer::basic(&username, &password))
        }
        Some(Authentication::BearerToken(token)) => {
            router.layer(AddAuthorizationLayer::bearer(&token))
        }
        Some(_) => return Err(miette!("Unsupported authentication method")),
        None => router,
    };

    let server = axum::serve(listener, router).into_future();
    let join_handle = tokio::spawn(server);

    let url = format!("http://{}/simple/", address);
    Ok((url, join_handle))
}
