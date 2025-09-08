//! Utilities to generate local PyPI indexes for tests.
//! - Flat (find-links) directory of wheels
//! - Simple (PEP 503) index with index.html pages

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
    pub timestamp: Option<DateTime<Utc>>, // Not embedded, but kept for parity/extension
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
}

/// A collection of packages that can be materialized as either flat or simple indexes.
#[derive(Default)]
pub struct Database {
    packages: Vec<PyPIPackage>,
}

impl Database {
    pub fn new() -> Self {
        Self { packages: vec![] }
    }

    pub fn add(&mut self, pkg: PyPIPackage) {
        self.packages.push(pkg);
    }

    pub fn with(mut self, pkg: PyPIPackage) -> Self {
        self.add(pkg);
        self
    }

    /// Writes all packages as wheels to a temporary directory and returns the flat index handle.
    pub fn into_flat_index(self) -> miette::Result<FlatIndex> {
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
        let mut projects: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for pkg in &self.packages {
            let project = normalize_simple_name(&pkg.name);
            let project_dir = index_root.join(&project);
            fs::create_dir_all(&project_dir).into_diagnostic()?;
            // write wheel inside project dir
            let wheel_path = write_wheel(&project_dir, pkg)?;
            projects.entry(project).or_default().push(
                wheel_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            );
        }

        // Write per-project index.html files
        const INDEX_TMPL: &str =
            "<!-- generated -->\n<!DOCTYPE html>\n<html><body>\n%LINKS%\n</body></html>\n";
        for (project, files) in &projects {
            let mut links = String::new();
            for fname in files {
                let _ = writeln!(links, "<a href=\"{}\">{}</a>", fname, fname);
            }
            let html = INDEX_TMPL.replace("%LINKS%", &links);
            fs::write(index_root.join(project).join("index.html"), html).into_diagnostic()?;
        }

        // Write root index.html linking to projects
        let mut proj_links = String::new();
        for project in projects.keys() {
            let _ = writeln!(proj_links, "<a href=\"/{p}\">{p}</a>", p = project);
        }
        let root_html = INDEX_TMPL.replace("%LINKS%", &proj_links);
        fs::write(index_root.join("index.html"), root_html).into_diagnostic()?;

        Ok(SimpleIndex {
            dir,
            index_root,
            _db: self,
        })
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
        &pkg.version,
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
        &pkg.version
    )
}

/// Build METADATA content.
fn build_metadata(pkg: &PyPIPackage) -> String {
    let mut s = String::new();
    s.push_str("Metadata-Version: 2.1\n");
    s.push_str(&format!("Name: {}\n", pkg.name));
    s.push_str(&format!("Version: {}\n", pkg.version));
    if let Some(summary) = &pkg.summary {
        s.push_str(&format!("Summary: {}\n", summary));
    }
    if let Some(rp) = &pkg.requires_python {
        s.push_str(&format!("Requires-Python: {}\n", rp));
    }
    for req in &pkg.requires_dist {
        s.push_str(&format!("Requires-Dist: {}\n", req));
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
    let path = format!("{}/__init__.py", module_dir);
    let bytes = b"# generated by pixi tests\n__version__ = \"".to_vec();
    let mut content = bytes;
    content.extend_from_slice(pkg.version.as_bytes());
    content.extend_from_slice(b"\"\n");
    (path, content)
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
    let metadata_path = format!("{}/METADATA", dist_info);
    entries.push((metadata_path.clone(), build_metadata(pkg).into_bytes()));

    // WHEEL
    let wheel_file_path = format!("{}/WHEEL", dist_info);
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
    let record_path = format!("{}/RECORD", dist_info);
    let _ = writeln!(record, "{record_path},,");

    // Write RECORD
    zip.start_file(&record_path, options).into_diagnostic()?;
    zip.write_all(record.as_bytes()).into_diagnostic()?;

    zip.finish().into_diagnostic()?;
    Ok(wheel_path)
}
