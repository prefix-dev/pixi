use std::{
    io,
    io::{Write, stdout},
};

use clap::Parser;
use console::Color;
use fancy_display::FancyDisplay;
use human_bytes::human_bytes;
use miette::IntoDiagnostic;
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
    if args.json || args.json_pretty {
        // print packages as json
        json_packages(&packages_to_output, args.json_pretty);
    } else {
        if !environment.is_default() {
            eprintln!("Environment: {}", environment.name().fancy_display());
        }

        // print packages as table
        print_packages_as_table(&packages_to_output)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                } else {
                    e
                }
            })
            .into_diagnostic()?;
    }

    Ok(())
}

fn print_packages_as_table(packages: &Vec<Package>) -> io::Result<()> {
    let mut writer = tabwriter::TabWriter::new(stdout());

    let header_style = console::Style::new().bold().cyan();
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
                    PackageKind::Conda =>
                        consts::CONDA_PACKAGE_STYLE.apply_to(&package.name).bold(),
                    PackageKind::Pypi => consts::PYPI_PACKAGE_STYLE.apply_to(&package.name).bold(),
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

        let fancy_kind = match package.kind {
            PackageKind::Conda => consts::CONDA_PACKAGE_STYLE.apply_to("conda"),
            PackageKind::Pypi => consts::PYPI_PACKAGE_STYLE.apply_to("pypi"),
        };

        writeln!(
            writer,
            "\t{}\t{}\t{}\t{}\t{}{}",
            &package.version,
            package.build.as_deref().unwrap_or(""),
            size_human,
            &fancy_kind,
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

fn json_packages(packages: &Vec<Package>, json_pretty: bool) {
    let json_string = if json_pretty {
        serde_json::to_string_pretty(&packages)
    } else {
        serde_json::to_string(&packages)
    }
    .expect("Cannot serialize packages to JSON");

    println!("{json_string}");
}
