use std::{
    borrow::Cow,
    io,
    io::{Write, stdout},
};

use clap::Parser;
use console::Color;
use fancy_display::FancyDisplay;
use human_bytes::human_bytes;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_consts::consts;
use pixi_core::{
    WorkspaceLocator,
    lock_file::{UpdateLockFileOptions, UvResolutionContext},
};
use pixi_manifest::FeaturesExt;
use pixi_uv_conversions::{
    ConversionError, pypi_options_to_index_locations, to_uv_normalize, to_uv_version,
};
use pypi_modifiers::pypi_tags::{get_pypi_tags, is_python_record};
use rattler_conda_types::Platform;
use rattler_lock::{CondaPackageData, LockedPackageRef, PypiPackageData, UrlOrPath};
use serde::Serialize;
use uv_configuration::ConfigSettings;
use uv_distribution::RegistryWheelIndex;

use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};

// an enum to sort by size or name
#[derive(clap::ValueEnum, Clone, Debug, Serialize)]
pub enum SortBy {
    Size,
    Name,
    Kind,
}

/// List workspace's packages.
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
    #[arg(long)]
    pub json: bool,

    /// Whether to output in pretty json format
    #[arg(long)]
    pub json_pretty: bool,

    /// Sorting strategy
    #[arg(long, default_value = "name", value_enum)]
    pub sort_by: SortBy,

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

fn serde_skip_is_editable(editable: &bool) -> bool {
    !(*editable)
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
    size_bytes: Option<u64>,
    kind: KindPackage,
    source: Option<String>,
    is_explicit: bool,
    #[serde(skip_serializing_if = "serde_skip_is_editable")]
    is_editable: bool,
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
            Self::PyPI(value, _) => value.version.to_string().into(),
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
        .and_then(|env| env.packages(platform).map(Vec::from_iter))
        .unwrap_or_default();

    let locked_deps_ext = locked_deps
        .into_iter()
        .map(|p| match p {
            LockedPackageRef::Pypi(pypi_data, _) => {
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
    let mut registry_index = if let Some(python_record) = python_record {
        if environment.has_pypi_dependencies() {
            uv_context = UvResolutionContext::from_workspace(&workspace)?;
            index_locations =
                pypi_options_to_index_locations(&environment.pypi_options(), workspace.root())
                    .into_diagnostic()?;
            tags = get_pypi_tags(
                platform,
                &environment.system_requirements(),
                python_record.record(),
            )?;
            Some(RegistryWheelIndex::new(
                &uv_context.cache,
                &tags,
                &index_locations,
                &uv_types::HashStrategy::None,
                &config_settings,
            ))
        } else {
            None
        }
    } else {
        None
    };

    // Get the explicit project dependencies
    let mut project_dependency_names = environment
        .combined_dependencies(Some(platform))
        .names()
        .map(|p| p.as_source().to_string())
        .collect_vec();
    project_dependency_names.extend(
        environment
            .pypi_dependencies(Some(platform))
            .into_iter()
            .map(|(name, _)| name.as_normalized().as_dist_info_name().into_owned()),
    );

    let mut packages_to_output = locked_deps_ext
        .iter()
        .map(|p| create_package_to_output(p, &project_dependency_names, registry_index.as_mut()))
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
    if args.json || args.json_pretty {
        // print packages as json
        json_packages(&packages_to_output, args.json_pretty);
    } else {
        if !environment.is_default() {
            eprintln!("Environment: {}", environment.name().fancy_display());
        }

        // print packages as table
        print_packages_as_table(&packages_to_output).expect("an io error occurred");
    }

    Ok(())
}

fn print_packages_as_table(packages: &Vec<PackageToOutput>) -> io::Result<()> {
    let mut writer = tabwriter::TabWriter::new(stdout());

    let header_style = console::Style::new().bold();
    writeln!(
        writer,
        "{}\t{}\t{}\t{}\t{}\t{}",
        header_style.apply_to("Package"),
        header_style.apply_to("Version"),
        header_style.apply_to("Build"),
        header_style.apply_to("Size"),
        header_style.apply_to("Kind"),
        header_style.apply_to("Source")
    )?;

    for package in packages {
        if package.is_explicit {
            write!(
                writer,
                "{}",
                match package.kind {
                    KindPackage::Conda =>
                        consts::CONDA_PACKAGE_STYLE.apply_to(&package.name).bold(),
                    KindPackage::Pypi => consts::PYPI_PACKAGE_STYLE.apply_to(&package.name).bold(),
                }
            )?
        } else {
            write!(writer, "{}", &package.name)?;
        };

        // Convert size to human readable format
        let size_human = package
            .size_bytes
            .map(|size| human_bytes(size as f64))
            .unwrap_or_default();

        writeln!(
            writer,
            "\t{}\t{}\t{}\t{}\t{}{}",
            &package.version,
            package.build.as_deref().unwrap_or(""),
            size_human,
            &package.kind.fancy_display(),
            package.source.as_deref().unwrap_or(""),
            if package.is_editable {
                format!(" {}", console::style("(editable)").fg(Color::Yellow))
            } else {
                "".to_string()
            }
        )?;
    }

    writer.flush()
}

fn json_packages(packages: &Vec<PackageToOutput>, json_pretty: bool) {
    let json_string = if json_pretty {
        serde_json::to_string_pretty(&packages)
    } else {
        serde_json::to_string(&packages)
    }
    .expect("Cannot serialize packages to JSON");

    println!("{}", json_string);
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
    project_dependency_names: &'a [String],
    registry_index: Option<&'a mut RegistryWheelIndex<'b>>,
) -> miette::Result<PackageToOutput> {
    let name = package.name().to_string();
    let version = package.version().into_owned();
    let kind = KindPackage::from(package);

    let build = match package {
        PackageExt::Conda(pkg) => Some(pkg.record().build.clone()),
        PackageExt::PyPI(_, _) => None,
    };

    let (size_bytes, source) = match package {
        PackageExt::Conda(pkg) => (
            pkg.record().size,
            match pkg {
                CondaPackageData::Source(source) => Some(source.location.to_string()),
                CondaPackageData::Binary(binary) => binary.channel.as_ref().map(|c| c.to_string()),
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

    Ok(PackageToOutput {
        name,
        version,
        build,
        size_bytes,
        kind,
        source,
        is_explicit,
        is_editable,
    })
}
