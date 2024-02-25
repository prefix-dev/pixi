use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use build_html::{Html, HtmlContainer};
use indoc::formatdoc;
use rattler_networking::Authentication;
use std::collections::BTreeSet;
use std::future::IntoFuture;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tower_http::add_extension::AddExtensionLayer;
use tower_http::auth::AddAuthorizationLayer;
use url::Url;
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

fn make_wheel(package_name: &str) -> anyhow::Result<Vec<u8>> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));

    let options = FileOptions::default();

    // Create METADATA file
    let metadata_filename = format!("{}-{}.dist-info/METADATA", package_name, WHEEL_VERSION);
    let metadata_content = make_manifest_file(package_name);
    zip.start_file(&metadata_filename, options)?;
    zip.write_all(metadata_content.as_bytes())?;

    // Create WHEEL file
    let wheel_filename = format!("{}-{}.dist-info/WHEEL", package_name, WHEEL_VERSION);
    let wheel_content = make_wheel_file();
    zip.start_file(&wheel_filename, options)?;
    zip.write_all(wheel_content.as_bytes())?;

    // Add __init__.py file
    let init_py_filename = format!("{}/{}", package_name, INIT_PY);
    zip.start_file(&init_py_filename, options)?;
    zip.write_all(b"")?;

    zip.finish()?;
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

    let wheel_name = format!("{}-{}-{}.whl", requested_package, WHEEL_VERSION, WHEEL_TAG);

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
    let (base_name, version, tag) = match parts.as_slice() {
        [base_name, version, tag] => (*base_name, *version, *tag),
        _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };

    if version != WHEEL_VERSION || tag != WHEEL_TAG {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }

    let base_name = base_name.to_string();
    if !served_packages.contains(&base_name) {
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
) -> anyhow::Result<(Url, JoinHandle<Result<(), std::io::Error>>)> {
    let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let address = listener.local_addr()?;

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
        Some(_) => return Err(anyhow::anyhow!("Unsupported authentication method")),
        None => router,
    };

    let server = axum::serve(listener, router).into_future();
    let join_handle = tokio::spawn(server);

    let url = format!("http://{}/simple/", address).parse()?;
    Ok((url, join_handle))
}
