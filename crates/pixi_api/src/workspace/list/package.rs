use std::borrow::Cow;
use std::collections::HashMap;

use pixi_uv_conversions::to_uv_version;
use rattler_lock::{CondaPackageData, PypiPackageData, UrlOrPath};
use serde::Serialize;
use uv_distribution::RegistryWheelIndex;

#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub build: Option<String>,
    pub build_number: Option<u64>,
    pub size_bytes: Option<u64>,
    pub kind: PackageKind,
    pub source: Option<String>,
    pub license: Option<String>,
    pub license_family: Option<String>,
    pub is_explicit: bool,
    #[serde(skip_serializing_if = "serde_skip_is_editable")]
    pub is_editable: bool,
    pub md5: Option<String>,
    pub sha256: Option<String>,
    pub arch: Option<String>,
    pub platform: Option<String>,
    pub subdir: Option<String>,
    pub timestamp: Option<i64>,
    pub noarch: Option<String>,
    pub file_name: Option<String>,
    pub url: Option<String>,
    pub requested_spec: Option<String>,
    pub constrains: Vec<String>,
    pub depends: Vec<String>,
    pub track_features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Conda,
    Pypi,
}

impl Package {
    pub(crate) fn new<'a, 'b>(
        package: &'b PackageExt,
        requested_specs: &'a HashMap<String, String>,
        registry_index: Option<&'a mut RegistryWheelIndex<'b>>,
    ) -> Self {
        let name = package.name().to_string();
        let version = package.version().into_owned();
        let kind = PackageKind::from(package);

        let build = match package {
            PackageExt::Conda(pkg) => Some(pkg.record().build.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let build_number = match package {
            PackageExt::Conda(pkg) => Some(pkg.record().build_number),
            PackageExt::PyPI(_, _) => None,
        };

        let (size_bytes, source) = match package {
            PackageExt::Conda(pkg) => (
                pkg.record().size,
                match pkg {
                    CondaPackageData::Source(source) => Some(source.location.to_string()),
                    CondaPackageData::Binary(binary) => binary
                        .channel
                        .as_ref()
                        .map(|c| c.to_string().trim_end_matches('/').to_string()),
                },
            ),
            PackageExt::PyPI(p, name) => {
                // Check the hash to avoid non index packages to be handled by the registry
                // index as wheels
                if p.hash.is_some() {
                    if let Some(registry_index) = registry_index {
                        // Handle case where the registry index is present
                        let entry = registry_index.get(name).find(|i| {
                            i.dist.filename.version
                                == to_uv_version(&p.version).expect("invalid version")
                        });
                        let size = entry.and_then(|e| get_dir_size(e.dist.path.clone()).ok());
                        let name = entry.map(|e| e.dist.filename.to_string());
                        (size, name)
                    } else {
                        get_pypi_location_information(&p.location)
                    }
                } else {
                    get_pypi_location_information(&p.location)
                }
            }
        };

        let license = match package {
            PackageExt::Conda(pkg) => pkg.record().license.clone(),
            PackageExt::PyPI(_, _) => None,
        };

        let license_family = match package {
            PackageExt::Conda(pkg) => pkg.record().license_family.clone(),
            PackageExt::PyPI(_, _) => None,
        };

        let md5 = match package {
            PackageExt::Conda(pkg) => pkg.record().md5.map(|h| format!("{h:x}")),
            PackageExt::PyPI(p, _) => p
                .hash
                .as_ref()
                .and_then(|h| h.md5().map(|m| format!("{m:x}"))),
        };

        let sha256 = match package {
            PackageExt::Conda(pkg) => pkg.record().sha256.map(|h| format!("{h:x}")),
            PackageExt::PyPI(p, _) => p
                .hash
                .as_ref()
                .and_then(|h| h.sha256().map(|s| format!("{s:x}"))),
        };

        let arch = match package {
            PackageExt::Conda(pkg) => pkg.record().arch.clone(),
            PackageExt::PyPI(_, _) => None,
        };

        let platform = match package {
            PackageExt::Conda(pkg) => pkg.record().platform.clone(),
            PackageExt::PyPI(_, _) => None,
        };

        let subdir = match package {
            PackageExt::Conda(pkg) => Some(pkg.record().subdir.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let timestamp = match package {
            PackageExt::Conda(pkg) => pkg.record().timestamp.map(|ts| ts.timestamp_millis()),
            PackageExt::PyPI(_, _) => None,
        };

        let noarch = match package {
            PackageExt::Conda(pkg) => {
                let noarch_type = &pkg.record().noarch;
                if noarch_type.is_python() {
                    Some("python".to_string())
                } else if noarch_type.is_generic() {
                    Some("generic".to_string())
                } else {
                    None
                }
            }
            PackageExt::PyPI(_, _) => None,
        };

        let (file_name, url) = match package {
            PackageExt::Conda(pkg) => match pkg {
                CondaPackageData::Binary(binary) => (
                    Some(binary.file_name.to_file_name()),
                    Some(binary.location.to_string()),
                ),
                CondaPackageData::Source(source) => (None, Some(source.location.to_string())),
            },
            PackageExt::PyPI(p, _) => match &p.location {
                UrlOrPath::Url(url) => (None, Some(url.to_string())),
                UrlOrPath::Path(path) => (None, Some(path.to_string())),
            },
        };

        let requested_spec = requested_specs.get(&name).cloned();
        let is_explicit = requested_spec.is_some();

        let is_editable = match package {
            PackageExt::Conda(_) => false,
            PackageExt::PyPI(p, _) => p.editable,
        };

        let constrains = match package {
            PackageExt::Conda(pkg) => pkg.record().constrains.clone(),
            PackageExt::PyPI(_, _) => Vec::new(),
        };

        let depends = match package {
            PackageExt::Conda(pkg) => pkg.record().depends.clone(),
            PackageExt::PyPI(p, _) => p.requires_dist.iter().map(|r| r.to_string()).collect(),
        };

        let track_features = match package {
            PackageExt::Conda(pkg) => pkg.record().track_features.clone(),
            PackageExt::PyPI(_, _) => Vec::new(),
        };

        Self {
            name,
            version,
            build,
            build_number,
            size_bytes,
            kind,
            source,
            license,
            license_family,
            is_explicit,
            is_editable,
            md5,
            sha256,
            arch,
            platform,
            subdir,
            timestamp,
            noarch,
            file_name,
            url,
            requested_spec,
            constrains,
            depends,
            track_features,
        }
    }
}

/// Return the size and source location of the pypi package
fn get_pypi_location_information(location: &UrlOrPath) -> (Option<u64>, Option<String>) {
    match location {
        UrlOrPath::Url(url) => (None, Some(url.to_string())),
        UrlOrPath::Path(path) => (
            get_dir_size(std::path::Path::new(path.as_str())).ok(),
            Some(path.to_string()),
        ),
    }
}

/// Get directory size
pub fn get_dir_size<P>(path: P) -> std::io::Result<u64>
where
    P: AsRef<std::path::Path>,
{
    let mut result = 0;

    if path.as_ref().is_dir() {
        for entry in fs_err::read_dir(path.as_ref())? {
            let _path = entry?.path();
            if _path.is_file() {
                result += _path.metadata()?.len();
            } else {
                result += get_dir_size(_path)?;
            }
        }
    } else {
        result = path.as_ref().metadata()?.len();
    }
    Ok(result)
}

fn serde_skip_is_editable(editable: &bool) -> bool {
    !(*editable)
}

/// Associate with a uv_normalize::PackageName
#[allow(clippy::large_enum_variant)]
pub(crate) enum PackageExt {
    PyPI(PypiPackageData, uv_normalize::PackageName),
    Conda(CondaPackageData),
}

impl From<&PackageExt> for PackageKind {
    fn from(package: &PackageExt) -> Self {
        match package {
            PackageExt::Conda(_) => PackageKind::Conda,
            PackageExt::PyPI(_, _) => PackageKind::Pypi,
        }
    }
}

impl PackageExt {
    pub fn as_conda(&self) -> Option<&CondaPackageData> {
        match self {
            PackageExt::Conda(c) => Some(c),
            _ => None,
        }
    }

    /// Returns the name of the package.
    pub fn name(&self) -> Cow<'_, str> {
        match self {
            Self::Conda(value) => value.record().name.as_normalized().into(),
            Self::PyPI(value, _) => value.name.as_dist_info_name(),
        }
    }

    /// Returns the version string of the package
    pub fn version(&self) -> Cow<'_, str> {
        match self {
            Self::Conda(value) => value.record().version.as_str(),
            Self::PyPI(value, _) => value.version.to_string().into(),
        }
    }
}
