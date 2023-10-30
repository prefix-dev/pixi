use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use rattler_conda_types::RepoDataRecord;
use reqwest::Response;
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Deserialize)]
struct CondaPyPiNameMapping {
    conda_name: String,
    pypi_name: String,
}

/// Determine the python packages that are installed as part of the conda packages.
/// TODO: Add some form of HTTP caching mechanisms here.
pub async fn find_conda_python_packages(
    records: &[RepoDataRecord],
) -> miette::Result<Vec<(rip::NormalizedPackageName, rip::Version)>> {
    // Get all the records from conda-forge
    let conda_forge_records = records
        .iter()
        .filter(|r| is_conda_forge_package(r))
        .collect_vec();

    // If there are none we can stop here
    if conda_forge_records.is_empty() {
        return Ok(Vec::new());
    }

    // Download the conda-forge pypi name mapping
    let response = reqwest::get("https://raw.githubusercontent.com/regro/cf-graph-countyfair/master/mappings/pypi/name_mapping.json")
        .await
        .and_then(Response::error_for_status)
        .into_diagnostic()
        .context("failed to download pypi name mapping")?;
    let mapping: Vec<CondaPyPiNameMapping> = response
        .json()
        .await
        .into_diagnostic()
        .context("failed to parse pypi name mapping")?;
    let mapping_by_name: HashMap<_, _> = mapping
        .into_iter()
        .map(|m| (m.conda_name, m.pypi_name))
        .collect();

    // Find python package names from the conda-forge package names
    let packages = conda_forge_records
        .iter()
        .filter_map(|r| {
            let pypi_name = mapping_by_name.get(r.package_record.name.as_normalized())?;
            let pypi_name = rip::NormalizedPackageName::from_str(pypi_name).ok()?;
            let version = rip::Version::from_str(&r.package_record.version.as_str()).ok()?;
            Some((pypi_name, version))
        })
        .collect();

    Ok(packages)
}

/// Returns `true` if the specified record refers to a conda-forge package.
fn is_conda_forge_package(record: &RepoDataRecord) -> bool {
    let channel = record.channel.as_str();
    channel.starts_with("https://conda.anaconda.org/conda-forge")
        || channel.starts_with("https://repo.prefix.dev/conda-forge")
}
