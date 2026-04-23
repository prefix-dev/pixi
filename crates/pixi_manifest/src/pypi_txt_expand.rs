//! Parse pip-style `requirements.txt` using the same stack as `pixi import --format=pypi-txt`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use dunce::canonicalize;
use pep508_rs::Requirement;
use tempfile::NamedTempFile;
use thiserror::Error;
use uv_client::BaseClientBuilder;
use uv_requirements_txt::RequirementsTxt;
use uv_requirements_txt::RequirementsTxtRequirement;

use crate::{TomlError, error::GenericError};

/// Errors from expanding `pypi-txt` requirements files.
#[derive(Debug, Error)]
pub enum PypiTxtExpandError {
    #[error(transparent)]
    RequirementsTxt(#[from] uv_requirements_txt::RequirementsTxtFileError),

    #[error("unnamed requirements are not supported in requirements files")]
    UnnamedRequirement,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("failed to join worker thread")]
    Join,

    #[error(transparent)]
    Pep508(#[from] pep508_rs::Pep508Error),

    #[error(
        "requirements file constraints (-c/--constraint) are not supported yet; remove constraint entries or fold pins into the main requirements file"
    )]
    UnsupportedConstraints,

    #[error(
        "requirements path `{}` must be located under the workspace directory `{}`",
        path.display(),
        workspace.display()
    )]
    PathOutsideWorkspace { path: PathBuf, workspace: PathBuf },
}

/// `-r` argument for a requirements file, quoted when the path contains spaces.
fn format_requirements_include_line(include_path: &Path) -> String {
    let s = include_path.to_string_lossy();
    if s.chars().any(|c| c.is_whitespace()) {
        format!(
            "-r \"{}\"",
            s.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("-r {s}")
    }
}

/// Resolve a manifest-listed requirements path to an absolute path and ensure it stays under
/// `workspace_root` after canonicalization (mitigates `..` and symlink escapes for entries
/// users list in `pypi-txt`). Nested `-r` inside those files are still resolved by UV with the
/// usual pip semantics.
fn resolve_pypi_txt_path_under_workspace(
    path: &Path,
    workspace_root: &Path,
) -> Result<PathBuf, PypiTxtExpandError> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    let canonical_workspace = canonicalize(workspace_root).map_err(PypiTxtExpandError::Io)?;
    let canonical_file = canonicalize(&joined).map_err(PypiTxtExpandError::Io)?;
    if !canonical_file.starts_with(&canonical_workspace) {
        return Err(PypiTxtExpandError::PathOutsideWorkspace {
            path: joined,
            workspace: canonical_workspace,
        });
    }
    Ok(canonical_file)
}

fn convert_uv_requirements_txt_to_pep508(
    reqs_txt: RequirementsTxt,
) -> Result<Vec<Requirement>, PypiTxtExpandError> {
    if !reqs_txt.constraints.is_empty() {
        return Err(PypiTxtExpandError::UnsupportedConstraints);
    }

    let uv_requirements: Vec<uv_pep508::Requirement<uv_pypi_types::VerbatimParsedUrl>> = reqs_txt
        .requirements
        .into_iter()
        .map(|r| match r.requirement {
            RequirementsTxtRequirement::Named(req) => Ok(req),
            RequirementsTxtRequirement::Unnamed(_) => Err(PypiTxtExpandError::UnnamedRequirement),
        })
        .collect::<Result<_, _>>()?;

    uv_requirements
        .iter()
        .map(|r| {
            let requirement = r.to_string();
            Requirement::from_str(&requirement).map_err(PypiTxtExpandError::Pep508)
        })
        .collect()
}

/// Parse a single requirements file (possibly including `-r` includes) into PEP 508 requirements.
pub async fn requirements_txt_to_requirements(
    requirements_path: &Path,
    workspace_root: &Path,
) -> Result<Vec<Requirement>, PypiTxtExpandError> {
    let reqs_txt = RequirementsTxt::parse(
        requirements_path,
        workspace_root,
        &BaseClientBuilder::default(),
    )
    .await?;
    convert_uv_requirements_txt_to_pep508(reqs_txt)
}

/// Parse multiple requirements paths (relative or absolute), in order, using `workspace_root`
/// as the UV workspace root (same as the manifest directory for pixi).
///
/// When more than one path is given, they are combined in a single UV parse (multiple `-r`
/// lines in a temporary file under `workspace_root`). That matches pip semantics and ensures
/// nested `-r` includes shared between those roots are only read once.
///
/// Each manifest-listed path is canonicalized and required to lie under `workspace_root`.
pub async fn pypi_txt_paths_to_requirements(
    paths: &[PathBuf],
    workspace_root: &Path,
) -> Result<Vec<Requirement>, PypiTxtExpandError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    // Canonicalize once so manifest-listed paths, `pathdiff`, and UV agree on the workspace
    // root (e.g. `/var/folders/...` vs `/private/var/...` on macOS).
    let workspace_root = canonicalize(workspace_root).map_err(PypiTxtExpandError::Io)?;

    let reqs_txt = if paths.len() == 1 {
        let abs = resolve_pypi_txt_path_under_workspace(&paths[0], &workspace_root)?;
        RequirementsTxt::parse(&abs, &workspace_root, &BaseClientBuilder::default()).await?
    } else {
        let mut tmp: NamedTempFile = tempfile::Builder::new()
            .prefix(".pixi-pypi-txt-union-")
            .suffix(".txt")
            .tempfile_in(&workspace_root)?;
        for path in paths {
            let abs = resolve_pypi_txt_path_under_workspace(path, &workspace_root)?;
            let include_arg = pathdiff::diff_paths(&abs, &workspace_root).unwrap_or(abs);
            writeln!(
                tmp.as_file_mut(),
                "{}",
                format_requirements_include_line(&include_arg)
            )?;
        }
        tmp.as_file_mut().flush()?;
        RequirementsTxt::parse(tmp.path(), &workspace_root, &BaseClientBuilder::default()).await?
    };

    convert_uv_requirements_txt_to_pep508(reqs_txt)
}

fn expand_blocking_inner(
    paths: &[PathBuf],
    workspace_root: &Path,
) -> Result<Vec<Requirement>, PypiTxtExpandError> {
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| -> Result<Vec<Requirement>, PypiTxtExpandError> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(pypi_txt_paths_to_requirements(paths, workspace_root))
        });
        match handle.join() {
            Ok(res) => res,
            Err(_) => Err(PypiTxtExpandError::Join),
        }
    })
}

/// Expand `pypi-txt` paths from a manifest using a dedicated thread + single-threaded runtime
/// so this stays safe to call from sync manifest loading (including under the CLI's tokio runtime).
pub fn expand_pypi_txt_paths_blocking(
    paths: &[PathBuf],
    manifest_dir: &Path,
) -> Result<Vec<Requirement>, TomlError> {
    expand_blocking_inner(paths, manifest_dir).map_err(|e| {
        TomlError::Generic(
            GenericError::new(format!("failed to parse `pypi-txt` requirements: {e}"))
                .with_help("See https://pixi.sh/latest/tutorials/import/ for the supported requirements file format (same as `pixi import --format=pypi-txt`)."),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_pypi_txt_roots_share_uv_include_graph() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs_err::write(root.join("common.txt"), "requests\n").unwrap();
        fs_err::write(root.join("a.txt"), "-r common.txt\nflask\n").unwrap();
        fs_err::write(root.join("b.txt"), "-r common.txt\npandas\n").unwrap();

        let paths = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        let expanded = expand_pypi_txt_paths_blocking(&paths, root).expect("expand");

        let request_count = expanded
            .iter()
            .filter(|r| r.name.as_ref() == "requests")
            .count();
        assert_eq!(
            request_count,
            1,
            "separate parses would list `requests` twice when both roots `-r` the same file"
        );
        assert!(expanded.iter().any(|r| r.name.as_ref() == "flask"));
        assert!(expanded.iter().any(|r| r.name.as_ref() == "pandas"));
    }

    #[test]
    fn manifest_pypi_txt_path_outside_workspace_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let unique = root.file_name().expect("dir name").to_string_lossy();
        let outside_file = format!("pixi-pypi-txt-outside-{unique}.txt");
        let outside = root.parent().expect("parent").join(outside_file);
        fs_err::write(&outside, "requests\n").unwrap();

        let err = expand_pypi_txt_paths_blocking(&[PathBuf::from(format!("../{}", outside.file_name().unwrap().to_string_lossy()))], root)
            .expect_err("outside path");
        let msg = err.to_string();
        assert!(
            msg.contains("pypi-txt") || msg.contains("requirements"),
            "{msg}"
        );
    }
}
