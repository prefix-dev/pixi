/// Some vendored structs and functions from
/// https://github.com/astral-sh/uv/tree/main/crates/install-wheel-rs
use csv::ReaderBuilder;

type WheelInfo = (Vec<RecordEntry>, PathBuf);

pub fn get_wheel_info(whl: &Path, venv: &PythonEnvironment) -> miette::Result<Option<WheelInfo>> {
    let dist_info_prefix = find_dist_info(whl)?;
    // Read the RECORD file.
    let mut record_file =
        File::open(whl.join(format!("{dist_info_prefix}.dist-info/RECORD"))).into_diagnostic()?;
    let records = read_record_file(&mut record_file)?;

    let whl_kind = get_wheel_kind(whl, dist_info_prefix).unwrap_or(LibKind::Unknown);

    let site_packages_dir = match whl_kind {
        LibKind::Unknown => return Ok(None),
        LibKind::Plat => venv.interpreter().virtualenv().platlib.clone(),
        LibKind::Pure => venv.interpreter().virtualenv().purelib.clone(),
    };

    Ok(Some((records, site_packages_dir)))
}

/// Find the `dist-info` directory in an unzipped wheel.
///
/// See: <https://github.com/PyO3/python-pkginfo-rs>
///
/// See: <https://github.com/pypa/pip/blob/36823099a9cdd83261fdbc8c1d2a24fa2eea72ca/src/pip/_internal/utils/wheel.py#L38>
fn find_dist_info(path: impl AsRef<Path>) -> miette::Result<String> {
    // Iterate over `path` to find the `.dist-info` directory. It should be at the top-level.
    let Some(dist_info) = fs::read_dir(path.as_ref())
        .into_diagnostic()?
        .find_map(|entry| {
            let entry = entry.ok()?;
            let file_type = entry.file_type().ok()?;
            if file_type.is_dir() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "dist-info") {
                    Some(path)
                } else {
                    None
                }
            } else {
                None
            }
        })
    else {
        miette::bail!("Missing .dist-info directory",);
    };

    let Some(dist_info_prefix) = dist_info.file_stem() else {
        miette::bail!("Missing .dist-info directory",);
    };

    Ok(dist_info_prefix.to_string_lossy().to_string())
}

/// Reads the record file
/// <https://www.python.org/dev/peps/pep-0376/#record>
pub fn read_record_file(record: &mut impl Read) -> miette::Result<Vec<RecordEntry>> {
    ReaderBuilder::new()
        .has_headers(false)
        .escape(Some(b'"'))
        .from_reader(record)
        .deserialize()
        .map(|entry| {
            let entry: RecordEntry = entry.into_diagnostic()?;
            Ok(RecordEntry {
                // selenium uses absolute paths for some reason
                path: entry.path.trim_start_matches('/').to_string(),
                ..entry
            })
        })
        .collect()
}

pub fn get_wheel_kind(wheel_path: &Path, dist_info_prefix: String) -> miette::Result<LibKind> {
    // We're going step by step though
    // https://packaging.python.org/en/latest/specifications/binary-distribution-format/#installing-a-wheel-distribution-1-0-py32-none-any-whl
    // > 1.a Parse distribution-1.0.dist-info/WHEEL.
    // > 1.b Check that installer is compatible with Wheel-Version. Warn if minor version is greater, abort if major version is greater.
    let wheel_file_path = wheel_path.join(format!("{dist_info_prefix}.dist-info/WHEEL"));
    let wheel_text = fs::read_to_string(wheel_file_path).into_diagnostic()?;
    let lib_kind = parse_wheel_file(&wheel_text)?;
    Ok(lib_kind)
}

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use miette::IntoDiagnostic;
use serde::{Deserialize, Serialize};
use uv_interpreter::PythonEnvironment;

/// Line in a RECORD file
/// <https://www.python.org/dev/peps/pep-0376/#record>
///
/// ```csv
/// tqdm/cli.py,sha256=x_c8nmc4Huc-lKEsAXj78ZiyqSJ9hJ71j7vltY67icw,10509
/// tqdm-4.62.3.dist-info/RECORD,,
/// ```
#[derive(Deserialize, Serialize, PartialOrd, PartialEq, Ord, Eq)]
pub(crate) struct RecordEntry {
    pub(crate) path: String,
    pub(crate) hash: Option<String>,
    #[allow(dead_code)]
    pub(crate) size: Option<u64>,
}

/// Whether the wheel should be installed into the `purelib` or `platlib` directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibKind {
    /// Install into the `purelib` directory.
    Pure,
    /// Install into the `platlib` directory.
    Plat,
    /// Unknown wheel kind when major version
    /// for `Wheel-Version: 1.0`
    /// is greater than 1
    Unknown,
}

/// Parse a file with `Key: value` entries such as WHEEL and METADATA
fn parse_key_value_file(
    file: impl Read,
    debug_filename: &str,
) -> miette::Result<HashMap<String, Vec<String>>> {
    let mut data: HashMap<String, Vec<String>> = HashMap::default();

    let file = BufReader::new(file);
    for (line_no, line) in file.lines().enumerate() {
        let line = line.into_diagnostic()?.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once(':').ok_or_else(|| {
            miette::miette!(
                "Line {} of the {debug_filename} file is invalid",
                line_no + 1
            )
        })?;
        data.entry(key.trim().to_string())
            .or_default()
            .push(value.trim().to_string());
    }
    Ok(data)
}

/// Parse WHEEL file.
///
/// > {distribution}-{version}.dist-info/WHEEL is metadata about the archive itself in the same
/// > basic key: value format:
pub(crate) fn parse_wheel_file(wheel_text: &str) -> miette::Result<LibKind> {
    // {distribution}-{version}.dist-info/WHEEL is metadata about the archive itself in the same basic key: value format:
    let data = parse_key_value_file(&mut wheel_text.as_bytes(), "WHEEL")?;

    // Determine whether Root-Is-Purelib == ‘true’.
    // If it is, the wheel is pure, and should be installed into purelib.
    let root_is_purelib = data
        .get("Root-Is-Purelib")
        .and_then(|root_is_purelib| root_is_purelib.first())
        .is_some_and(|root_is_purelib| root_is_purelib == "true");
    let lib_kind = if root_is_purelib {
        LibKind::Pure
    } else {
        LibKind::Plat
    };

    // mkl_fft-1.3.6-58-cp310-cp310-manylinux2014_x86_64.whl has multiple Wheel-Version entries, we have to ignore that
    // like pip
    let wheel_version = data
        .get("Wheel-Version")
        .and_then(|wheel_versions| wheel_versions.first());
    let wheel_version = wheel_version
        .and_then(|wheel_version| wheel_version.split_once('.'))
        .ok_or_else(|| miette::miette!("Invalid Wheel-Version in WHEEL file: {wheel_version:?}"))?;

    // pip has some test wheels that use that ancient version,
    // and technically we only need to check that the version is not higher
    if wheel_version == ("0", "1") {
        return Ok(lib_kind);
    }
    // Check that installer is compatible with Wheel-Version. Warn if minor version is greater, abort if major version is greater.
    // Wheel-Version: 1.0
    if wheel_version.0 != "1" {
        return Ok(LibKind::Unknown);
    }
    Ok(lib_kind)
}
