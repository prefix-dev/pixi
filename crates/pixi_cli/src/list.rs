use std::{borrow::Cow, collections::HashMap, fmt::Display};

use clap::Parser;
use comfy_table::{Cell, CellAlignment, ContentArrangement, Table, presets::NOTHING};
use console::Style;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_consts::consts;
use pixi_core::{WorkspaceLocator, lock_file::UpdateLockFileOptions};
use pixi_manifest::FeaturesExt;
use pixi_uv_context::UvResolutionContext;
use pixi_uv_conversions::{
    ConversionError, pypi_options_to_index_locations, to_uv_normalize, to_uv_version,
};
use pypi_modifiers::pypi_tags::{get_pypi_tags, is_python_record};
use rattler_conda_types::Platform;
use rattler_lock::{CondaPackageData, LockedPackageRef, PypiPackageData, UrlOrPath};
use serde::Serialize;
use uv_client::{Connectivity, MetadataFormat, OwnedArchive, RegistryClient};
use uv_configuration::IndexStrategy;
use uv_distribution::RegistryWheelIndex;
use uv_distribution_types::{
    ConfigSettings, ExtraBuildRequires, ExtraBuildVariables, IndexCapabilities, IndexFormat,
    IndexMetadataRef, IndexUrl, PackageConfigSettings,
};
use uv_pep508::VerbatimUrl;
use uv_pypi_types::HashAlgorithm;
use uv_redacted::DisplaySafeUrl;

use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};

// an enum to sort by size or name
#[derive(clap::ValueEnum, Clone, Debug, Serialize)]
pub enum SortBy {
    Size,
    Name,
    Kind,
}

/// Available fields for the list command output
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Arch,
    Build,
    #[clap(name = "build-number")]
    BuildNumber,
    Constrains,
    Depends,
    #[clap(name = "file-name")]
    FileName,
    Kind,
    License,
    #[clap(name = "license-family")]
    LicenseFamily,
    Md5,
    Name,
    Noarch,
    Platform,
    #[clap(name = "requested-spec")]
    RequestedSpec,
    Sha256,
    Size,
    Source,
    Subdir,
    Timestamp,
    #[clap(name = "track-features")]
    TrackFeatures,
    Url,
    Version,
}

impl Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Use lowercase for ValueEnum compatibility
        match self {
            Field::Arch => write!(f, "arch"),
            Field::Build => write!(f, "build"),
            Field::BuildNumber => write!(f, "build-number"),
            Field::Constrains => write!(f, "constrains"),
            Field::Depends => write!(f, "depends"),
            Field::FileName => write!(f, "file-name"),
            Field::Kind => write!(f, "kind"),
            Field::License => write!(f, "license"),
            Field::LicenseFamily => write!(f, "license-family"),
            Field::Md5 => write!(f, "md5"),
            Field::Name => write!(f, "name"),
            Field::Noarch => write!(f, "noarch"),
            Field::Platform => write!(f, "platform"),
            Field::RequestedSpec => write!(f, "requested-spec"),
            Field::Sha256 => write!(f, "sha256"),
            Field::Size => write!(f, "size"),
            Field::Source => write!(f, "source"),
            Field::Subdir => write!(f, "subdir"),
            Field::Timestamp => write!(f, "timestamp"),
            Field::TrackFeatures => write!(f, "track-features"),
            Field::Url => write!(f, "url"),
            Field::Version => write!(f, "version"),
        }
    }
}

impl Field {
    /// Get the header display name for this field (used in table output)
    fn header_name(&self) -> &'static str {
        match self {
            Field::Arch => "Arch",
            Field::Build => "Build",
            Field::BuildNumber => "Build#",
            Field::Constrains => "Constrains",
            Field::Depends => "Depends",
            Field::FileName => "File Name",
            Field::Kind => "Kind",
            Field::License => "License",
            Field::LicenseFamily => "License Family",
            Field::Md5 => "MD5",
            Field::Name => "Name",
            Field::Noarch => "Noarch",
            Field::Platform => "Platform",
            Field::RequestedSpec => "Requested",
            Field::Sha256 => "SHA256",
            Field::Size => "Size",
            Field::Source => "Source",
            Field::Subdir => "Subdir",
            Field::Timestamp => "Timestamp",
            Field::TrackFeatures => "Track Features",
            Field::Url => "URL",
            Field::Version => "Version",
        }
    }

    /// Get the cell alignment for this field
    fn alignment(&self) -> Option<CellAlignment> {
        match self {
            Field::Size | Field::BuildNumber | Field::Timestamp => Some(CellAlignment::Right),
            _ => None,
        }
    }

    /// Create a styled header cell for this field
    fn header_cell(&self, style: &Style) -> Cell {
        let mut cell = Cell::new(format!("{}", style.apply_to(self.header_name())));
        if let Some(align) = self.alignment() {
            cell = cell.set_alignment(align);
        }
        cell
    }
}

/// Default fields to display when --fields is not specified
pub const DEFAULT_FIELDS: [Field; 6] = [
    Field::Name,
    Field::Version,
    Field::Build,
    Field::Size,
    Field::Kind,
    Field::Source,
];

/// List the packages of the current workspace
///
/// Highlighted packages are explicit dependencies.
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    /// List only packages matching a regular expression
    #[arg()]
    pub regex: Option<String>,

    /// The platform to list packages for. Defaults to the current platform.
    #[arg(long)]
    pub platform: Option<Platform>,

    /// Whether to output in json format
    #[arg(long, alias = "json-pretty")]
    pub json: bool,

    /// Sorting strategy
    #[arg(long, default_value = "name", value_enum, conflicts_with = "json")]
    pub sort_by: SortBy,

    /// Select which fields to display and in what order (comma-separated).
    #[arg(long, value_delimiter = ',', default_values_t = DEFAULT_FIELDS, conflicts_with = "json")]
    pub fields: Vec<Field>,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The environment to list packages for. Defaults to the default
    /// environment.
    #[arg(short, long)]
    pub environment: Option<String>,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    /// Only list packages that are explicitly defined in the workspace.
    #[arg(short = 'x', long)]
    pub explicit: bool,
}

#[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
enum KindPackage {
    Conda,
    Pypi,
}

impl From<&PackageExt> for KindPackage {
    fn from(package: &PackageExt) -> Self {
        match package {
            PackageExt::Conda(_) => KindPackage::Conda,
            PackageExt::PyPI(_, _) => KindPackage::Pypi,
        }
    }
}

impl FancyDisplay for KindPackage {
    fn fancy_display(&self) -> console::StyledObject<&str> {
        match self {
            KindPackage::Conda => consts::CONDA_PACKAGE_STYLE.apply_to("conda"),
            KindPackage::Pypi => consts::PYPI_PACKAGE_STYLE.apply_to("pypi"),
        }
    }
}

#[derive(Serialize)]
struct PackageToOutput {
    name: String,
    version: String,
    build: Option<String>,
    build_number: Option<u64>,
    size_bytes: Option<u64>,
    kind: KindPackage,
    source: Option<String>,
    license: Option<String>,
    license_family: Option<String>,
    is_explicit: bool,
    md5: Option<String>,
    sha256: Option<String>,
    arch: Option<String>,
    platform: Option<String>,
    subdir: Option<String>,
    timestamp: Option<i64>,
    noarch: Option<String>,
    file_name: Option<String>,
    url: Option<String>,
    requested_spec: Option<String>,
    constrains: Vec<String>,
    depends: Vec<String>,
    track_features: Vec<String>,
}

impl PackageToOutput {
    /// Get a Cell for a field, with proper styling and alignment
    fn get_field_cell(&self, field: Field) -> Cell {
        let mut cell = match field {
            Field::Name => {
                let content = if self.is_explicit {
                    let style = match self.kind {
                        KindPackage::Conda => consts::CONDA_PACKAGE_STYLE.clone().bold(),
                        KindPackage::Pypi => consts::PYPI_PACKAGE_STYLE.clone().bold(),
                    };
                    format!("{}", style.apply_to(&self.name))
                } else {
                    self.name.clone()
                };
                Cell::new(content)
            }
            Field::Version => Cell::new(&self.version),
            Field::Build => Cell::new(self.build.as_deref().unwrap_or_default()),
            Field::BuildNumber => {
                Cell::new(self.build_number.map(|n| n.to_string()).unwrap_or_default())
            }
            Field::Size => Cell::new(
                self.size_bytes
                    .map(|size| indicatif::HumanBytes(size).to_string())
                    .unwrap_or_default(),
            ),
            Field::Kind => Cell::new(format!("{}", self.kind.fancy_display())),
            Field::Source => Cell::new(self.source.as_deref().unwrap_or_default().to_string()),
            Field::License => Cell::new(self.license.as_deref().unwrap_or_default()),
            Field::LicenseFamily => Cell::new(self.license_family.as_deref().unwrap_or_default()),
            Field::Md5 => Cell::new(self.md5.as_deref().unwrap_or_default()),
            Field::Sha256 => Cell::new(self.sha256.as_deref().unwrap_or_default()),
            Field::Arch => Cell::new(self.arch.as_deref().unwrap_or_default()),
            Field::Platform => Cell::new(self.platform.as_deref().unwrap_or_default()),
            Field::Subdir => Cell::new(self.subdir.as_deref().unwrap_or_default()),
            Field::Timestamp => {
                Cell::new(self.timestamp.map(|t| t.to_string()).unwrap_or_default())
            }
            Field::Noarch => Cell::new(self.noarch.as_deref().unwrap_or_default()),
            Field::FileName => Cell::new(self.file_name.as_deref().unwrap_or_default()),
            Field::Url => Cell::new(self.url.as_deref().unwrap_or_default()),
            Field::RequestedSpec => Cell::new(self.requested_spec.as_deref().unwrap_or_default()),
            Field::Constrains => Cell::new(self.constrains.join(", ")),
            Field::Depends => Cell::new(self.depends.join(", ")),
            Field::TrackFeatures => Cell::new(self.track_features.join(", ")),
        };

        if let Some(align) = field.alignment() {
            cell = cell.set_alignment(align);
        }
        cell
    }
}

/// Get directory size
pub(crate) fn get_dir_size<P>(path: P) -> std::io::Result<u64>
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

/// Metadata extracted from the cached Simple API index for a single file.
struct PypiIndexFileInfo {
    size: Option<u64>,
    filename: String,
    upload_time_utc_ms: Option<i64>,
}

/// Look up PyPI file metadata from cached Simple API index data.
///
/// Groups packages by (name, index_url) so each query targets the
/// specific index that served the package, then builds a
/// sha256-hex → file-info map for matching in output.
async fn fetch_pypi_file_info_from_cache(
    packages: &[PackageExt],
    registry_client: &RegistryClient,
    capabilities: &IndexCapabilities,
) -> HashMap<String, PypiIndexFileInfo> {
    let mut info_map = HashMap::new();
    let semaphore = tokio::sync::Semaphore::new(50);

    let mut queries: HashMap<IndexUrl, Vec<uv_normalize::PackageName>> = HashMap::new();
    for p in packages {
        let PackageExt::PyPI(data, name) = p else {
            continue;
        };
        let Some(index_url) = &data.index_url else {
            continue;
        };
        if data.hash.is_none() {
            continue;
        }
        let index_url = IndexUrl::from(VerbatimUrl::from_url(DisplaySafeUrl::from(
            index_url.clone(),
        )));
        queries.entry(index_url).or_default().push(name.clone());
    }

    for (index_url, packages) in &queries {
        let index_hint = IndexMetadataRef {
            url: index_url,
            format: IndexFormat::Simple,
        };

        for name in packages {
            match registry_client
                .package_metadata(name, Some(index_hint), capabilities, &semaphore)
                .await
            {
                Ok(results) => {
                    for (result_index_url, metadata) in &results {
                        if *result_index_url != index_url {
                            continue;
                        }
                        let MetadataFormat::Simple(archive) = metadata else {
                            continue;
                        };
                        let metadata = OwnedArchive::deserialize(archive);
                        for datum in metadata {
                            collect_file_info(&datum.files, &mut info_map);
                        }
                    }
                }
                Err(err) => {
                    tracing::debug!("Failed to fetch cached metadata for {name}: {err}");
                    continue;
                }
            };
        }
    }

    info_map
}

/// Extract sha256 → file info mappings from a set of version files.
fn collect_file_info(
    files: &uv_client::VersionFiles,
    info_map: &mut HashMap<String, PypiIndexFileInfo>,
) {
    for wheel in &files.wheels {
        insert_file_info(&wheel.file, info_map);
    }
    for sdist in &files.source_dists {
        insert_file_info(&sdist.file, info_map);
    }
}

/// If the file has a sha256 hash, record its metadata.
fn insert_file_info(
    file: &uv_distribution_types::File,
    info_map: &mut HashMap<String, PypiIndexFileInfo>,
) {
    for hash in file.hashes.iter() {
        if hash.algorithm() == HashAlgorithm::Sha256 {
            info_map.insert(
                hash.digest.to_string(),
                PypiIndexFileInfo {
                    size: file.size,
                    filename: file.filename.to_string(),
                    upload_time_utc_ms: file.upload_time_utc_ms,
                },
            );
        }
    }
}

/// Associate with a uv_normalize::PackageName
#[allow(clippy::large_enum_variant)]
enum PackageExt {
    PyPI(PypiPackageData, uv_normalize::PackageName),
    Conda(CondaPackageData),
}

impl PackageExt {
    fn as_conda(&self) -> Option<&CondaPackageData> {
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
            Self::PyPI(value, _) => value.version_string().into(),
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let environment = workspace.environment_from_name_or_env_var(args.environment)?;

    let lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
            no_install: args.no_install_config.no_install,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await?
        .0
        .into_lock_file();

    // Load the platform
    let platform = args.platform.unwrap_or_else(|| environment.best_platform());

    // Get all the packages in the environment.
    let locked_deps = lock_file
        .environment(environment.name().as_str())
        .and_then(|env| {
            let p = lock_file.platform(&platform.to_string())?;
            env.packages(p).map(Vec::from_iter)
        })
        .unwrap_or_default();

    let locked_deps_ext = locked_deps
        .into_iter()
        .map(|p| match p {
            LockedPackageRef::Pypi(pypi_data) => {
                let name = to_uv_normalize(&pypi_data.name)?;
                Ok(PackageExt::PyPI(pypi_data.clone(), name))
            }
            LockedPackageRef::Conda(c) => Ok(PackageExt::Conda(c.clone())),
        })
        .collect::<Result<Vec<_>, ConversionError>>()
        .into_diagnostic()?;

    // Get the python record from the lock file
    let mut conda_records = locked_deps_ext.iter().filter_map(|d| d.as_conda());

    // Construct the registry index if we have a python record
    let python_record = conda_records.find(|r| is_python_record(r));
    let tags;
    let uv_context;
    let index_locations;
    let config_settings = ConfigSettings::default();
    let package_config_settings = PackageConfigSettings::default();
    let extra_build_requires = ExtraBuildRequires::default();
    let extra_build_variables = ExtraBuildVariables::default();

    let (mut registry_index, pypi_file_info) = if let Some(python_record) = python_record {
        if environment.has_pypi_dependencies() {
            uv_context = UvResolutionContext::from_config(workspace.config())?;
            index_locations =
                pypi_options_to_index_locations(&environment.pypi_options(), workspace.root())
                    .into_diagnostic()?;
            tags = get_pypi_tags(
                platform,
                &environment.system_requirements(),
                python_record.record(),
            )?;

            // Look up PyPI file sizes from cached Simple API data
            let registry_client = uv_context.build_registry_client(
                uv_context.allow_insecure_host.clone(),
                &index_locations,
                IndexStrategy::default(),
                None,
                Connectivity::Offline,
            );
            let pypi_file_info = fetch_pypi_file_info_from_cache(
                &locked_deps_ext,
                &registry_client,
                &uv_context.capabilities,
            )
            .await;

            let idx = RegistryWheelIndex::new(
                &uv_context.cache,
                &tags,
                &index_locations,
                &uv_types::HashStrategy::None,
                &config_settings,
                &package_config_settings,
                &extra_build_requires,
                &extra_build_variables,
            );
            (Some(idx), pypi_file_info)
        } else {
            (None, HashMap::new())
        }
    } else {
        (None, HashMap::new())
    };

    // Get the explicit project dependencies with their requested specs
    let mut requested_specs: HashMap<String, String> = environment
        .combined_dependencies(Some(platform))
        .iter()
        .map(|(name, specs)| {
            let spec_str = specs.iter().map(|s| s.to_string()).join(", ");
            (name.as_source().to_string(), spec_str)
        })
        .collect();
    requested_specs.extend(
        environment
            .pypi_dependencies(Some(platform))
            .into_iter()
            .map(|(name, reqs)| {
                let spec = reqs
                    .first()
                    .map(|r| r.to_string())
                    .unwrap_or_else(|| "*".to_string());
                (name.as_normalized().as_dist_info_name().into_owned(), spec)
            }),
    );

    let mut packages_to_output = locked_deps_ext
        .iter()
        .map(|p| {
            create_package_to_output(
                p,
                &requested_specs,
                registry_index.as_mut(),
                &pypi_file_info,
            )
        })
        .collect::<Result<Vec<PackageToOutput>, _>>()?;

    // Filter packages by regex if needed
    if let Some(regex) = args.regex {
        let regex = regex::Regex::new(&regex).map_err(|_| miette::miette!("Invalid regex"))?;
        packages_to_output = packages_to_output
            .into_iter()
            .filter(|p| regex.is_match(&p.name))
            .collect::<Vec<_>>();
    }

    // Filter packages by explicit if needed
    if args.explicit {
        packages_to_output = packages_to_output
            .into_iter()
            .filter(|p| p.is_explicit)
            .collect::<Vec<_>>();
    }

    // Sort according to the sorting strategy
    match args.sort_by {
        SortBy::Size => {
            packages_to_output
                .sort_by(|a, b| a.size_bytes.unwrap_or(0).cmp(&b.size_bytes.unwrap_or(0)));
        }
        SortBy::Name => {
            packages_to_output.sort_by(|a, b| a.name.cmp(&b.name));
        }
        SortBy::Kind => {
            packages_to_output.sort_by(|a, b| a.kind.cmp(&b.kind));
        }
    }

    if packages_to_output.is_empty() {
        miette::bail!(
            "No packages found in '{}' environment for '{}' platform.",
            environment.name().fancy_display(),
            consts::ENVIRONMENT_STYLE.apply_to(platform),
        );
    }

    // Print as table string or JSON
    if args.json {
        // print packages as json
        json_packages(&packages_to_output);
    } else {
        if !environment.is_default() {
            eprintln!("Environment: {}", environment.name().fancy_display());
        }

        // print packages as table
        print_packages_as_table(&packages_to_output, &args.fields);
    }

    Ok(())
}

fn print_packages_as_table(packages: &[PackageToOutput], fields: &[Field]) {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Disabled);

    // Set up header row
    let header_style = Style::new().bold().cyan();
    table.set_header(fields.iter().map(|f| f.header_cell(&header_style)));

    // Add each package row
    for package in packages {
        table.add_row(fields.iter().map(|f| package.get_field_cell(*f)));
    }

    println!(
        "{}",
        table
            .lines()
            .map(|line| line.trim().to_string())
            .format("\n")
    );
}

fn json_packages(packages: &Vec<PackageToOutput>) {
    let json_string =
        serde_json::to_string_pretty(&packages).expect("Cannot serialize packages to JSON");
    println!("{json_string}");
}

/// Look up cached index metadata for a PyPI package by its sha256 hash.
fn lookup_pypi_file_info<'a>(
    pkg: &PypiPackageData,
    pypi_file_info: &'a HashMap<String, PypiIndexFileInfo>,
) -> Option<&'a PypiIndexFileInfo> {
    pkg.hash
        .as_ref()
        .and_then(|h| h.sha256())
        .and_then(|sha| pypi_file_info.get(&format!("{sha:x}")))
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

fn create_package_to_output<'a, 'b>(
    package: &'b PackageExt,
    requested_specs: &'a HashMap<String, String>,
    registry_index: Option<&'a mut RegistryWheelIndex<'b>>,
    pypi_file_info: &HashMap<String, PypiIndexFileInfo>,
) -> miette::Result<PackageToOutput> {
    let name = package.name().to_string();
    let version = package.version().into_owned();
    let kind = KindPackage::from(package);

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
            let index_info = lookup_pypi_file_info(p, pypi_file_info);
            let source = p
                .index_url
                .as_ref()
                .map(|u| u.as_str().trim_end_matches('/').to_string());
            if p.hash.is_some() {
                if let Some(registry_index) = registry_index {
                    let entry = registry_index.get(name).find(|i| {
                        i.dist.filename.version
                            == to_uv_version(
                                p.version
                                    .as_ref()
                                    .expect("registry packages always have a version"),
                            )
                            .expect("invalid version")
                    });
                    let size = entry
                        .and_then(|e| get_dir_size(e.dist.path.clone()).ok())
                        .or_else(|| index_info.and_then(|i| i.size));
                    (size, source)
                } else {
                    let size = index_info.and_then(|i| i.size);
                    let fallback_size = get_pypi_location_information(&p.location).0;
                    (size.or(fallback_size), source)
                }
            } else {
                let (size, fallback_source) = get_pypi_location_information(&p.location);
                (size, source.or(fallback_source))
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
        PackageExt::PyPI(p, _) => {
            lookup_pypi_file_info(p, pypi_file_info).and_then(|i| i.upload_time_utc_ms)
        }
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
        PackageExt::PyPI(p, _) => {
            let file_name = lookup_pypi_file_info(p, pypi_file_info)
                .map(|i| i.filename.clone())
                .or_else(|| {
                    // Fall back to extracting the filename from the URL
                    match &*p.location {
                        UrlOrPath::Url(url) => {
                            url.path_segments().and_then(|s| s.last()).map(|s| {
                                percent_encoding::percent_decode_str(s)
                                    .decode_utf8()
                                    .map_or_else(|_| s.to_string(), |d| d.into_owned())
                            })
                        }
                        UrlOrPath::Path(_) => None,
                    }
                });
            let url = match &*p.location {
                UrlOrPath::Url(url) => Some(url.to_string()),
                UrlOrPath::Path(path) => Some(path.to_string()),
            };
            (file_name, url)
        }
    };

    let requested_spec = requested_specs.get(&name).cloned();
    let is_explicit = requested_spec.is_some();

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

    Ok(PackageToOutput {
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
    })
}
