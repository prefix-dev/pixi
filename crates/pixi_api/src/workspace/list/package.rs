use pixi_consts::consts;
use pixi_core::lock_file::HasNameVersion;
use pixi_install_pypi::UnresolvedPypiRecord;
use pixi_uv_conversions::to_uv_version;
use rattler_lock::{CondaPackageData, UrlOrPath};
use serde::Serialize;
use std::str::FromStr;
use std::{borrow::Cow, collections::HashMap};
use uv_distribution::RegistryWheelIndex;
use uv_distribution_filename::WheelFilename;
use uv_distribution_types::IndexUrl;
use uv_pep508::VerbatimUrl;

#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub name: String,
    pub version: Option<String>,
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
    pub index_url: Option<String>,
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
        let version = package.version();
        let kind = PackageKind::from(package);

        let build = match package {
            PackageExt::Conda(pkg) => pkg.record().map(|r| r.build.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let build_number = match package {
            PackageExt::Conda(pkg) => pkg.record().map(|r| r.build_number),
            PackageExt::PyPI(_, _) => None,
        };

        let (size_bytes, source) = match package {
            PackageExt::Conda(pkg) => (
                pkg.record().and_then(|r| r.size),
                match pkg {
                    CondaPackageData::Source(source) => Some(source.location.to_string()),
                    CondaPackageData::Binary(binary) => binary
                        .channel
                        .as_ref()
                        .map(|c| c.to_string().trim_end_matches('/').to_string()),
                },
            ),
            PackageExt::PyPI(record, name) => {
                let p = record.as_package_data();
                if p.hash.is_some() {
                    let url = p
                        .index_url
                        .clone()
                        .unwrap_or_else(|| consts::DEFAULT_PYPI_INDEX_URL.clone());
                    let index = IndexUrl::from(VerbatimUrl::from(url));
                    let size = if let Some(registry_index) = registry_index {
                        // Handle case where the registry index is present
                        let wheel_filename = p
                            .location
                            .file_name()
                            .and_then(|f| WheelFilename::from_str(f).ok());
                        let entry = registry_index.get(name).find(|entry| {
                            if entry.index.url() != &index {
                                return false;
                            }
                            if let Some(filename) = &wheel_filename {
                                &entry.dist.filename == filename
                            } else if let Some(version) = &p.version {
                                Some(&entry.dist.filename.version)
                                    == to_uv_version(version).ok().as_ref()
                            } else {
                                false
                            }
                        });
                        entry.and_then(|e| get_dir_size(&e.dist.path).ok())
                    } else {
                        get_pypi_location_information(&p.location).0
                    };
                    (size, Some(index.to_string()))
                } else {
                    get_pypi_location_information(&p.location)
                }
            }
        };

        let license = match package {
            PackageExt::Conda(pkg) => pkg.record().and_then(|r| r.license.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let license_family = match package {
            PackageExt::Conda(pkg) => pkg.record().and_then(|r| r.license_family.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let md5 = match package {
            PackageExt::Conda(pkg) => pkg.record().and_then(|r| r.md5.map(|h| format!("{h:x}"))),
            PackageExt::PyPI(record, _) => record
                .as_package_data()
                .hash
                .as_ref()
                .and_then(|h| h.md5().map(|m| format!("{m:x}"))),
        };

        let sha256 = match package {
            PackageExt::Conda(pkg) => pkg
                .record()
                .and_then(|r| r.sha256.map(|h| format!("{h:x}"))),
            PackageExt::PyPI(record, _) => record
                .as_package_data()
                .hash
                .as_ref()
                .and_then(|h| h.sha256().map(|s| format!("{s:x}"))),
        };

        let arch = match package {
            PackageExt::Conda(pkg) => pkg.record().and_then(|r| r.arch.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let platform = match package {
            PackageExt::Conda(pkg) => pkg.record().and_then(|r| r.platform.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let subdir = match package {
            PackageExt::Conda(pkg) => pkg.record().map(|r| r.subdir.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let timestamp = match package {
            PackageExt::Conda(pkg) => pkg
                .record()
                .and_then(|r| r.timestamp.map(|ts| ts.timestamp_millis())),
            PackageExt::PyPI(_, _) => None,
        };

        let noarch = match package {
            PackageExt::Conda(pkg) => pkg.record().and_then(|r| {
                let noarch_type = &r.noarch;
                if noarch_type.is_python() {
                    Some("python".to_string())
                } else if noarch_type.is_generic() {
                    Some("generic".to_string())
                } else {
                    None
                }
            }),
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
            PackageExt::PyPI(record, _) => {
                let p = record.as_package_data();
                (
                    None,
                    Some(
                        p.location
                            .given()
                            .map_or_else(|| p.location.to_string(), ToOwned::to_owned),
                    ),
                )
            }
        };

        let index_url = match package {
            PackageExt::PyPI(record, _) => record
                .as_package_data()
                .index_url
                .as_ref()
                .map(|u| u.to_string()),
            PackageExt::Conda(_) => None,
        };

        let requested_spec = requested_specs.get(&name).cloned();
        let is_explicit = requested_spec.is_some();

        let is_editable = match package {
            PackageExt::Conda(_) => false,
            PackageExt::PyPI(_p, _) => {
                // TODO: Should be derived from the input specs.
                false
            }
        };

        let constrains = match package {
            PackageExt::Conda(pkg) => pkg
                .record()
                .map(|r| r.constrains.clone())
                .unwrap_or_default(),
            PackageExt::PyPI(_, _) => Vec::new(),
        };

        let depends = match package {
            PackageExt::Conda(pkg) => pkg.record().map(|r| r.depends.clone()).unwrap_or_default(),
            PackageExt::PyPI(record, _) => record
                .as_package_data()
                .requires_dist
                .iter()
                .map(|r| r.to_string())
                .collect(),
        };

        let track_features = match package {
            PackageExt::Conda(pkg) => pkg
                .record()
                .map(|r| r.track_features.clone())
                .unwrap_or_default(),
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
            index_url,
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
    PyPI(UnresolvedPypiRecord, uv_normalize::PackageName),
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
            Self::Conda(value) => value.name().as_normalized().into(),
            Self::PyPI(value, _) => value.name().as_dist_info_name(),
        }
    }

    /// Returns the version string of the package
    pub fn version(&self) -> Option<String> {
        match self {
            Self::Conda(value) => value.record().map(|r| r.version.to_string()),
            Self::PyPI(value, _) => value.version().map(|v| v.to_string()),
        }
    }
}
