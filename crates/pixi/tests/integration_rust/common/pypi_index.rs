//! Utilities to generate local PyPI indexes for tests.
//! - Flat (find-links) directory of wheels
//! - Simple (PEP 503) index with index.html pages
//! - Simple index served over HTTP, so packages lock as registry URLs

#![allow(dead_code)]

use std::borrow::Cow;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs_err as fs;
use miette::IntoDiagnostic;
use tempfile::TempDir;
use url::Url;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

/// A single file listed on a project's simple (PEP 503) index page, plus the
/// metadata rendered into its link: an optional `data-upload-time` attribute
/// and an optional `#sha256=...` URL fragment.
struct ProjectFileEntry {
    filename: String,
    timestamp: Option<DateTime<Utc>>,
    sha256: Option<String>,
}

/// A wheel tag triple: (python tag, abi tag, platform tag).
/// Defaults to `py3-none-any`.
#[derive(Clone, Debug)]
pub struct WheelTag {
    pub py: String,
    pub abi: String,
    pub plat: String,
}

impl Default for WheelTag {
    fn default() -> Self {
        Self {
            py: "py3".to_string(),
            abi: "none".to_string(),
            plat: "any".to_string(),
        }
    }
}

/// Description of a fake PyPI package to be emitted as a wheel.
#[derive(Clone, Debug)]
pub struct PyPIPackage {
    pub name: String,
    pub version: String,
    pub tag: WheelTag,
    pub requires_dist: Vec<String>,
    pub requires_python: Option<String>,
    pub summary: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
}

impl PyPIPackage {
    /// Start building a package (defaults to `py3-none-any`).
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            tag: WheelTag::default(),
            requires_dist: vec![],
            requires_python: None,
            summary: None,
            timestamp: None,
        }
    }

    pub fn with_tag(
        mut self,
        py: impl Into<String>,
        abi: impl Into<String>,
        plat: impl Into<String>,
    ) -> Self {
        self.tag = WheelTag {
            py: py.into(),
            abi: abi.into(),
            plat: plat.into(),
        };
        self
    }

    pub fn with_requires_dist(mut self, reqs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.requires_dist = reqs.into_iter().map(|s| s.into()).collect();
        self
    }

    pub fn with_requires_python(mut self, spec: impl Into<String>) -> Self {
        self.requires_python = Some(spec.into());
        self
    }

    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }
}

/// A collection of packages that can be materialized as either flat or simple indexes.
#[derive(Default)]
pub struct Database {
    packages: Vec<PyPIPackage>,
    include_sha256: bool,
}

impl Database {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, pkg: PyPIPackage) {
        self.packages.push(pkg);
    }

    pub fn with(mut self, pkg: PyPIPackage) -> Self {
        self.add(pkg);
        self
    }

    /// Annotate the simple index links with `#sha256=...` fragments, like a
    /// real registry does. Resolving against such an index records the
    /// digests in the lock file.
    ///
    /// Only meaningful for [`Self::into_simple_index`] /
    /// [`Self::into_http_index`]; a flat (find-links) directory has no link
    /// pages to carry digests, so [`Self::into_flat_index`] rejects it.
    pub fn with_sha256_hashes(mut self) -> Self {
        self.include_sha256 = true;
        self
    }

    /// Writes all packages as wheels to a temporary directory and returns the flat index handle.
    pub fn into_flat_index(self) -> miette::Result<FlatIndex> {
        assert!(
            !self.include_sha256,
            "with_sha256_hashes() has no effect on a flat (find-links) index; \
             use into_simple_index() or into_http_index() instead"
        );
        let dir = TempDir::new().into_diagnostic()?;
        for pkg in &self.packages {
            write_wheel(dir.path(), pkg)?;
        }
        Ok(FlatIndex { dir, _db: self })
    }

    /// Writes packages into a simple (PEP 503) index layout under a temp dir.
    pub fn into_simple_index(self) -> miette::Result<SimpleIndex> {
        let dir = TempDir::new().into_diagnostic()?;
        let index_root = dir.path().join("index");
        fs::create_dir_all(&index_root).into_diagnostic()?;

        // Group wheels by normalized project name
        use std::collections::BTreeMap;
        let mut projects: BTreeMap<String, Vec<ProjectFileEntry>> = BTreeMap::new();

        for pkg in &self.packages {
            let project = normalize_simple_name(&pkg.name);
            let project_dir = index_root.join(&project);
            fs::create_dir_all(&project_dir).into_diagnostic()?;
            // write wheel inside project dir
            let wheel_path = write_wheel(&project_dir, pkg)?;
            let sha256 = if self.include_sha256 {
                let digest =
                    rattler_digest::compute_file_digest::<rattler_digest::Sha256>(&wheel_path)
                        .into_diagnostic()?;
                Some(format!("{digest:x}"))
            } else {
                None
            };
            projects.entry(project).or_default().push(ProjectFileEntry {
                filename: wheel_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
                timestamp: pkg.timestamp,
                sha256,
            });
        }

        // Write per-project index.html files
        const INDEX_TMPL: &str =
            "<!-- generated -->\n<!DOCTYPE html>\n<html><body>\n%LINKS%\n</body></html>\n";
        for (project, files) in &projects {
            let mut links = String::new();
            for entry in files {
                let fname = &entry.filename;
                let upload_time = entry
                    .timestamp
                    .map(|timestamp| format!(" data-upload-time=\"{}\"", timestamp.to_rfc3339()))
                    .unwrap_or_default();
                let fragment = entry
                    .sha256
                    .as_ref()
                    .map(|sha256| format!("#sha256={sha256}"))
                    .unwrap_or_default();
                let _ = writeln!(
                    links,
                    "<a href=\"{fname}{fragment}\"{upload_time}>{fname}</a>"
                );
            }
            let html = INDEX_TMPL.replace("%LINKS%", &links);
            fs::write(index_root.join(project).join("index.html"), html).into_diagnostic()?;
        }

        // Write root index.html linking to projects
        let mut proj_links = String::new();
        for project in projects.keys() {
            let _ = writeln!(proj_links, "<a href=\"/{project}\">{project}</a>");
        }
        let root_html = INDEX_TMPL.replace("%LINKS%", &proj_links);
        fs::write(index_root.join("index.html"), root_html).into_diagnostic()?;

        // Keep the digests queryable so tests don't have to re-hash wheels
        // or hard-code the index's on-disk layout.
        let digests = projects
            .into_values()
            .flatten()
            .filter_map(|entry| Some((entry.filename, entry.sha256?)))
            .collect();

        Ok(SimpleIndex {
            dir,
            index_root,
            digests,
            _db: self,
        })
    }

    /// Materialize the simple index and serve it over HTTP on an ephemeral
    /// local port, mimicking a real registry. Locked packages then carry a
    /// registry URL instead of a local path (and, combined with
    /// [`Self::with_sha256_hashes`], a digest).
    pub async fn into_http_index(self) -> miette::Result<HttpIndex> {
        HttpIndex::serve(self.into_simple_index()?).await
    }
}

/// A local flat index (find-links) represented by a directory of wheel files.
pub struct FlatIndex {
    dir: TempDir,
    _db: Database,
}

impl FlatIndex {
    /// Path to the directory containing wheels.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A `file://` URL pointing to the directory.
    pub fn url(&self) -> Url {
        Url::from_directory_path(self.dir.path()).expect("absolute path")
    }
}

/// Normalize a project name following PEP 503 (simple repository API).
/// Lowercase and replace runs of `[-_.]` with `-`.
fn normalize_simple_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_dash = false;
    for ch in lower.chars() {
        let is_sep = ch == '-' || ch == '_' || ch == '.';
        if is_sep {
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
        } else {
            out.push(ch);
            last_dash = false;
        }
    }
    out
}

/// A local simple (PEP 503) index.
pub struct SimpleIndex {
    dir: TempDir,
    index_root: PathBuf,
    /// sha256 hex digest per wheel filename, populated when the database was
    /// built with [`Database::with_sha256_hashes`].
    digests: std::collections::BTreeMap<String, String>,
    _db: Database,
}

impl SimpleIndex {
    /// Path to the `index` root directory.
    pub fn index_path(&self) -> &Path {
        &self.index_root
    }

    /// file:// URL pointing to the `index` root directory.
    pub fn index_url(&self) -> Url {
        Url::from_directory_path(&self.index_root).expect("absolute path")
    }

    /// The sha256 hex digest the index advertises for a package's wheel.
    /// Requires [`Database::with_sha256_hashes`] (panics otherwise, so a
    /// misconfigured test fails at the lookup rather than on a bad assert).
    pub fn wheel_sha256(&self, name: &str, version: &str) -> &str {
        let filename = wheel_filename(&PyPIPackage::new(name, version));
        self.digests.get(&filename).unwrap_or_else(|| {
            panic!(
                "no sha256 recorded for {filename}; was the database built \
                 with with_sha256_hashes() and does the package exist?"
            )
        })
    }
}

/// A simple (PEP 503) index served over HTTP on a local ephemeral port.
///
/// The server task lives as long as this handle and is aborted on drop.
pub struct HttpIndex {
    index: SimpleIndex,
    url: Url,
    server: tokio::task::JoinHandle<()>,
}

impl HttpIndex {
    async fn serve(index: SimpleIndex) -> miette::Result<Self> {
        use axum::{Router, routing::get};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .into_diagnostic()?;
        let addr = listener.local_addr().into_diagnostic()?;

        let root = index.index_path().to_path_buf();
        let router = Router::new().fallback(get(move |uri: axum::http::Uri| {
            let root = root.clone();
            async move { serve_index_file(&root, uri.path()) }
        }));

        let server = tokio::spawn(async move {
            // A panic here would vanish inside the detached task; print the
            // failure so it shows up next to the (otherwise opaque)
            // connection error the test will subsequently hit.
            if let Err(err) = axum::serve(listener, router).await {
                eprintln!("pypi index server failed: {err}");
            }
        });

        let url = Url::parse(&format!("http://{addr}/")).into_diagnostic()?;
        Ok(Self { index, url, server })
    }

    /// The root URL of the served index, usable as an `index-url`.
    pub fn index_url(&self) -> Url {
        self.url.clone()
    }

    /// Path to the underlying simple index on disk.
    pub fn index_path(&self) -> &Path {
        self.index.index_path()
    }

    /// The sha256 hex digest the index advertises for a package's wheel.
    /// See [`SimpleIndex::wheel_sha256`].
    pub fn wheel_sha256(&self, name: &str, version: &str) -> &str {
        self.index.wheel_sha256(name, version)
    }
}

impl Drop for HttpIndex {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Resolve a request path inside the simple index directory: directories are
/// served through their `index.html`, wheels as raw bytes.
///
/// Segments are percent-decoded (clients request e.g. `%2B` for the `+` in
/// local-version wheel filenames) and dot-segments are resolved per RFC 3986,
/// without ever escaping the index root.
fn serve_index_file(root: &Path, request_path: &str) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{Response, StatusCode, header::CONTENT_TYPE};

    let mut segments: Vec<String> = Vec::new();
    for raw in request_path.split('/') {
        let segment = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
        match segment.as_ref() {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment.to_string()),
        }
    }

    let mut path = root.to_path_buf();
    for segment in &segments {
        path.push(segment);
    }
    if path.is_dir() {
        path.push("index.html");
    }

    match fs::read(&path) {
        Ok(bytes) => {
            let content_type = if path.extension().is_some_and(|ext| ext == "html") {
                "text/html"
            } else {
                "application/octet-stream"
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, content_type)
                .body(Body::from(bytes))
                .unwrap()
        }
        Err(_) => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap(),
    }
}

/// Create a normalized distribution name for filenames (PEP 427): replace '-' with '_'.
fn normalize_dist_name(name: &str) -> Cow<'_, str> {
    if name.contains('-') {
        Cow::Owned(name.replace('-', "_"))
    } else {
        Cow::Borrowed(name)
    }
}

/// Construct a wheel filename: `{name}-{version}-{py}-{abi}-{plat}.whl`.
fn wheel_filename(pkg: &PyPIPackage) -> String {
    format!(
        "{}-{}-{}-{}-{}.whl",
        normalize_dist_name(&pkg.name),
        pkg.version,
        pkg.tag.py,
        pkg.tag.abi,
        pkg.tag.plat
    )
}

/// dist-info directory name: `{name}-{version}.dist-info` (normalized name).
fn dist_info_dir(pkg: &PyPIPackage) -> String {
    format!(
        "{}-{}.dist-info",
        normalize_dist_name(&pkg.name),
        pkg.version
    )
}

/// Build METADATA content.
fn build_metadata(pkg: &PyPIPackage) -> String {
    let mut s = String::new();
    s.push_str("Metadata-Version: 2.1\n");
    s.push_str(&format!("Name: {}\n", pkg.name));
    s.push_str(&format!("Version: {}\n", pkg.version));
    if let Some(summary) = &pkg.summary {
        s.push_str(&format!("Summary: {summary}\n"));
    }
    if let Some(rp) = &pkg.requires_python {
        s.push_str(&format!("Requires-Python: {rp}\n"));
    }
    for req in &pkg.requires_dist {
        s.push_str(&format!("Requires-Dist: {req}\n"));
    }
    s
}

/// Build WHEEL content.
fn build_wheel_file(pkg: &PyPIPackage) -> String {
    let mut s = String::new();
    s.push_str("Wheel-Version: 1.0\n");
    s.push_str("Generator: pixi-tests\n");
    s.push_str("Root-Is-Purelib: true\n");
    s.push_str(&format!(
        "Tag: {}-{}-{}\n",
        pkg.tag.py, pkg.tag.abi, pkg.tag.plat
    ));
    s
}

/// Write a minimal Python module file content for the package.
fn build_module(pkg: &PyPIPackage) -> (String, Vec<u8>) {
    let module_dir = normalize_dist_name(&pkg.name).to_string();
    let path = format!("{module_dir}/__init__.py");
    let bytes = b"# generated by pixi tests\n__version__ = \"".to_vec();
    let mut content = bytes;
    content.extend_from_slice(pkg.version.as_bytes());
    content.extend_from_slice(b"\"\n");
    (path, content)
}

/// Write a malformed wheel where the filename version doesn't match the METADATA version.
/// This is used to test the UV_SKIP_WHEEL_FILENAME_CHECK environment variable.
pub fn write_malformed_wheel(
    out_dir: &Path,
    filename_version: &str,
    metadata_version: &str,
    name: &str,
) -> miette::Result<PathBuf> {
    let wheel_name = format!(
        "{}-{}-py3-none-any.whl",
        normalize_dist_name(name),
        filename_version
    );
    let wheel_path = out_dir.join(&wheel_name);

    let file = std::fs::File::create(&wheel_path).into_diagnostic()?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();

    let dist_info = format!(
        "{}-{}.dist-info",
        normalize_dist_name(name),
        metadata_version
    );

    // METADATA with different version than filename
    let metadata = format!(
        "Metadata-Version: 2.1\nName: {}\nVersion: {}\nSummary: Malformed test wheel\n",
        name, metadata_version
    );

    // WHEEL file
    let wheel_content = "Wheel-Version: 1.0\nGenerator: pixi-tests-malformed\nRoot-Is-Purelib: true\nTag: py3-none-any\n";

    // Module file
    let module_dir = normalize_dist_name(name).to_string();
    let module_path = format!("{module_dir}/__init__.py");
    let module_content = format!(
        "# malformed test package\n__version__ = \"{}\"\n",
        metadata_version
    );

    // Write module
    zip.start_file(&module_path, options).into_diagnostic()?;
    zip.write_all(module_content.as_bytes()).into_diagnostic()?;

    // Write METADATA
    let metadata_path = format!("{dist_info}/METADATA");
    zip.start_file(&metadata_path, options).into_diagnostic()?;
    zip.write_all(metadata.as_bytes()).into_diagnostic()?;

    // Write WHEEL
    let wheel_file_path = format!("{dist_info}/WHEEL");
    zip.start_file(&wheel_file_path, options)
        .into_diagnostic()?;
    zip.write_all(wheel_content.as_bytes()).into_diagnostic()?;

    // Build and write RECORD
    let record_path = format!("{dist_info}/RECORD");
    let record =
        format!("{module_path},,\n{metadata_path},,\n{wheel_file_path},,\n{record_path},,\n");
    zip.start_file(&record_path, options).into_diagnostic()?;
    zip.write_all(record.as_bytes()).into_diagnostic()?;

    zip.finish().into_diagnostic()?;
    Ok(wheel_path)
}

/// Write a wheel to `out_dir` for the package.
fn write_wheel(out_dir: &Path, pkg: &PyPIPackage) -> miette::Result<PathBuf> {
    let wheel_name = wheel_filename(pkg);
    let wheel_path = out_dir.join(&wheel_name);

    let file = std::fs::File::create(&wheel_path).into_diagnostic()?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();

    let dist_info = dist_info_dir(pkg);

    // Prepare files to include so we can compute RECORD entries.
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();

    // Module file
    let (module_path, module_bytes) = build_module(pkg);
    entries.push((module_path, module_bytes));

    // METADATA
    let metadata_path = format!("{dist_info}/METADATA");
    entries.push((metadata_path.clone(), build_metadata(pkg).into_bytes()));

    // WHEEL
    let wheel_file_path = format!("{dist_info}/WHEEL");
    entries.push((wheel_file_path.clone(), build_wheel_file(pkg).into_bytes()));

    // Write all prepared entries to the zip
    for (name, bytes) in &entries {
        zip.start_file(name, options).into_diagnostic()?;
        use std::io::Write as _;
        zip.write_all(bytes).into_diagnostic()?;
    }

    // Build RECORD content. Omit hashes and sizes (allowed by PEP 376) for simplicity.
    let mut record = String::new();
    for (name, _bytes) in &entries {
        let _ = writeln!(record, "{name},,");
    }
    // RECORD line itself
    let record_path = format!("{dist_info}/RECORD");
    let _ = writeln!(record, "{record_path},,");

    // Write RECORD
    zip.start_file(&record_path, options).into_diagnostic()?;
    zip.write_all(record.as_bytes()).into_diagnostic()?;

    zip.finish().into_diagnostic()?;
    Ok(wheel_path)
}
