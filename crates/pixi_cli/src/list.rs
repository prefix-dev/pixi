use std::fmt::Display;

use clap::Parser;
use comfy_table::{Cell, CellAlignment, ContentArrangement, Table, presets::NOTHING};
use console::Style;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use pixi_api::{
    WorkspaceContext,
    workspace::{Package, PackageKind},
};
use pixi_consts::consts;
use pixi_core::WorkspaceLocator;
use rattler_conda_types::Platform;
use serde::Serialize;

use crate::{
    cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig},
    cli_interface::CliInterface,
};

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
    #[clap(name = "is-editable")]
    IsEditable,
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
            Field::IsEditable => write!(f, "is-editable"),
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
            Field::IsEditable => "Editable",
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

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let lock_file_usage = args.lock_file_update_config.lock_file_usage()?;
    let environment = workspace.environment_from_name_or_env_var(args.environment.clone())?;
    let platform = args.platform.unwrap_or_else(|| environment.best_platform());

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace.clone());
    let mut packages_to_output = workspace_ctx
        .list_packages(
            args.regex,
            args.platform,
            args.environment,
            args.explicit,
            args.no_install_config.no_install,
            lock_file_usage,
        )
        .await?;

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

/// Get a Cell for a field from a Package, with proper styling and alignment
fn get_field_cell(package: &Package, field: Field) -> Cell {
    let mut cell = match field {
        Field::Name => {
            let content = if package.is_explicit {
                let style = match package.kind {
                    PackageKind::Conda => consts::CONDA_PACKAGE_STYLE.clone().bold(),
                    PackageKind::Pypi => consts::PYPI_PACKAGE_STYLE.clone().bold(),
                };
                format!("{}", style.apply_to(&package.name))
            } else {
                package.name.clone()
            };
            Cell::new(content)
        }
        Field::Version => Cell::new(&package.version),
        Field::Build => Cell::new(package.build.as_deref().unwrap_or_default()),
        Field::BuildNumber => Cell::new(
            package
                .build_number
                .map(|n| n.to_string())
                .unwrap_or_default(),
        ),
        Field::Size => Cell::new(
            package
                .size_bytes
                .map(|size| indicatif::HumanBytes(size).to_string())
                .unwrap_or_default(),
        ),
        Field::Kind => {
            let fancy_kind = match package.kind {
                PackageKind::Conda => consts::CONDA_PACKAGE_STYLE.apply_to("conda"),
                PackageKind::Pypi => consts::PYPI_PACKAGE_STYLE.apply_to("pypi"),
            };
            Cell::new(format!("{}", fancy_kind))
        }
        Field::Source => {
            let base = package.source.as_deref().unwrap_or_default();
            let content = if package.is_editable {
                format!("{base} {}", Style::new().yellow().apply_to("(editable)"))
            } else {
                base.to_string()
            };
            Cell::new(content)
        }
        Field::License => Cell::new(package.license.as_deref().unwrap_or_default()),
        Field::LicenseFamily => Cell::new(package.license_family.as_deref().unwrap_or_default()),
        Field::IsEditable => Cell::new(if package.is_editable { "true" } else { "false" }),
        Field::Md5 => Cell::new(package.md5.as_deref().unwrap_or_default()),
        Field::Sha256 => Cell::new(package.sha256.as_deref().unwrap_or_default()),
        Field::Arch => Cell::new(package.arch.as_deref().unwrap_or_default()),
        Field::Platform => Cell::new(package.platform.as_deref().unwrap_or_default()),
        Field::Subdir => Cell::new(package.subdir.as_deref().unwrap_or_default()),
        Field::Timestamp => Cell::new(package.timestamp.map(|t| t.to_string()).unwrap_or_default()),
        Field::Noarch => Cell::new(package.noarch.as_deref().unwrap_or_default()),
        Field::FileName => Cell::new(package.file_name.as_deref().unwrap_or_default()),
        Field::Url => Cell::new(package.url.as_deref().unwrap_or_default()),
        Field::RequestedSpec => Cell::new(package.requested_spec.as_deref().unwrap_or_default()),
        Field::Constrains => Cell::new(package.constrains.join(", ")),
        Field::Depends => Cell::new(package.depends.join(", ")),
        Field::TrackFeatures => Cell::new(package.track_features.join(", ")),
    };

    if let Some(align) = field.alignment() {
        cell = cell.set_alignment(align);
    }
    cell
}

fn print_packages_as_table(packages: &[Package], fields: &[Field]) {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Disabled);

    // Set up header row
    let header_style = Style::new().bold().cyan();
    table.set_header(fields.iter().map(|f| f.header_cell(&header_style)));

    // Add each package row
    for package in packages {
        table.add_row(fields.iter().map(|f| get_field_cell(package, *f)));
    }

    println!(
        "{}",
        table
            .lines()
            .map(|line| line.trim().to_string())
            .format("\n")
    );
}

fn json_packages(packages: &Vec<Package>) {
    let json_string =
        serde_json::to_string_pretty(&packages).expect("Cannot serialize packages to JSON");
    println!("{json_string}");
}
