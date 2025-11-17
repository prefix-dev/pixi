use std::borrow::Cow;

use pixi_uv_conversions::to_uv_version;
use rattler_lock::{CondaPackageData, PypiPackageData, UrlOrPath};
use serde::Serialize;
use uv_distribution::RegistryWheelIndex;

#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub build: Option<String>,
    pub size_bytes: Option<u64>,
    pub kind: PackageKind,
    pub source: Option<String>,
    pub is_explicit: bool,
    #[serde(skip_serializing_if = "serde_skip_is_editable")]
    pub is_editable: bool,
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
        project_dependency_names: &'a [String],
        registry_index: Option<&'a mut RegistryWheelIndex<'b>>,
    ) -> Self {
        let name = package.name().to_string();
        let version = package.version().into_owned();
        let kind = PackageKind::from(package);

        let build = match package {
            PackageExt::Conda(pkg) => Some(pkg.record().build.clone()),
            PackageExt::PyPI(_, _) => None,
        };

        let (size_bytes, source) = match package {
            PackageExt::Conda(pkg) => (
                pkg.record().size,
                match pkg {
                    CondaPackageData::Source(source) => Some(source.location.to_string()),
                    CondaPackageData::Binary(binary) => {
                        binary.channel.as_ref().map(|c| c.to_string())
                    }
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

        let is_explicit = project_dependency_names.contains(&name);
        let is_editable = match package {
            PackageExt::Conda(_) => false,
            PackageExt::PyPI(p, _) => p.editable,
        };

        Self {
            name,
            version,
            build,
            size_bytes,
            kind,
            source,
            is_explicit,
            is_editable,
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
