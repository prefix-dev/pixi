use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use human_bytes::human_bytes;
use indexmap::IndexSet;
use itertools::Itertools;
use miette::{IntoDiagnostic, miette};
use pixi_api::global::{
    EnvironmentName, GlobalDependency, GlobalEnvironment, GlobalExposedMapping, Project,
    report::{self, EnvReport, Item, Label, Marker, Row},
};
use pixi_config::{Config, ConfigCli};
use pixi_core::environment::list::{PackageToOutput, print_package_table};
use rattler_conda_types::PackageName;
use serde::Serialize;
use std::str::FromStr;

use crate::cli_interface::cli_global_context;

/// Lists global environments with their dependencies and exposed commands. Can also display all packages within a specific global environment when using the --environment flag.
///
/// All environments:
///
/// - Magenta: the name of the environment.
/// - Green: the packages that are explicit dependencies of the environment.
/// - Yellow: the binaries that are exposed.
/// - Dim: the version of the installed package.
///
/// Per environment:
///
/// - Green: packages that are explicitly installed.
#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    /// List only packages matching a regular expression.
    /// Without regex syntax it acts like a `contains` filter.
    #[arg()]
    pub regex: Option<String>,

    #[clap(flatten)]
    config: ConfigCli,

    /// Allows listing all the packages installed in a specific environment, with an output similar to `pixi list`.
    #[clap(short, long)]
    environment: Option<String>,

    /// Sorting strategy for the package table of an environment
    #[arg(long, default_value = "name", value_enum, requires = "environment")]
    sort_by: GlobalSortBy,

    /// Whether to output in JSON format
    #[arg(long)]
    json: bool,
}

/// Sorting strategy for the package table
#[derive(clap::ValueEnum, Clone, Debug, Default, PartialEq)]
pub enum GlobalSortBy {
    Size,
    #[default]
    Name,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());
    let context = cli_global_context(project);

    if args.json {
        let env_name = args
            .environment
            .map(|e| EnvironmentName::from_str(e.as_str()))
            .transpose()?;
        let mut environments = context.list_environments(args.regex).await?;
        if let Some(env_name) = env_name {
            environments.retain(|env| env.name == env_name);
            if environments.is_empty() {
                return Err(miette!(
                    "environment {} not found",
                    env_name.fancy_display()
                ));
            }
        }
        return print_environments_json(&environments);
    }

    if let Some(environment) = args.environment {
        let env_name = EnvironmentName::from_str(environment.as_str())?;
        // Verify that the environment is in sync with the manifest and report to the user otherwise
        if !context.project().environment_in_sync(&env_name).await? {
            tracing::warn!(
                "The environment {} is not in sync with the manifest, to sync run\n\tpixi global sync",
                env_name.fancy_display()
            );
        }

        let environment = context.get_environment(&env_name).await?;
        print_specific_global_environment(
            &environment,
            args.sort_by == GlobalSortBy::Size,
            args.regex,
        )?;
    } else {
        // Verify that the environments are in sync with the manifest and report to the user otherwise
        if !context.project().environments_in_sync().await? {
            tracing::warn!(
                "The environments are not in sync with the manifest, to sync run\n\tpixi global sync"
            );
        }
        let environments = context.list_environments(args.regex).await?;
        print_all_global_environments(&environments)?;
    }

    Ok(())
}

/// JSON-serializable representation of a global environment
#[derive(Serialize)]
struct GlobalEnvironmentJson {
    name: String,
    dependencies: Vec<GlobalDependency>,
    exposed: Vec<GlobalExposedMapping>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
}

/// Print global environments as JSON to stdout
fn print_environments_json(environments: &[GlobalEnvironment]) -> miette::Result<()> {
    let environments = environments
        .iter()
        .map(|env| GlobalEnvironmentJson {
            name: env.name.as_str().to_string(),
            dependencies: env.dependencies.clone(),
            exposed: env.exposed.clone(),
            platform: env.platform.map(|p| p.to_string()),
        })
        .collect_vec();

    let json_string =
        serde_json::to_string_pretty(&environments).expect("cannot serialize environments to JSON");
    write_stdout(&json_string)
}

/// Write a line to stdout, treating a closed pipe as a normal end of output
fn write_stdout(line: &str) -> miette::Result<()> {
    pixi_utils::io::ignore_broken_pipe(writeln!(std::io::stdout(), "{line}")).into_diagnostic()
}

fn format_mapping(mapping: &GlobalExposedMapping) -> Item {
    let detail =
        (mapping.exposed_name != mapping.executable).then(|| format!("-> {}", mapping.executable));
    Item::exposed(Marker::None, mapping.exposed_name.clone(), detail)
}

/// The row listing what an environment exposes, or that it exposes nothing
fn exposed_row(exposed: &[GlobalExposedMapping]) -> Row {
    if exposed.is_empty() {
        Row::new(Label::Exposed, vec![Item::summary("nothing")])
    } else {
        Row::new(Label::Exposed, exposed.iter().map(format_mapping).collect())
    }
}

/// The row naming the dependencies of an environment with their installed
/// versions. An environment holding a single dependency named after itself
/// carries it in the header instead
fn dependencies_row(environment: &GlobalEnvironment) -> Option<Row> {
    if !environment
        .dependencies
        .iter()
        .any(|dep| dep.name.as_normalized() != environment.name.as_str())
    {
        return None;
    }

    let items = environment
        .dependencies
        .iter()
        .sorted_by_key(|dep| dep.name.as_normalized())
        .map(|dep| Item::package(Marker::None, dep.name.as_normalized(), dep.version.clone()))
        .collect();

    Some(Row::new(Label::Dependencies, items))
}

fn shortcuts_row(shortcuts: &IndexSet<PackageName>) -> Option<Row> {
    (!shortcuts.is_empty()).then(|| {
        Row::new(
            Label::Shortcuts,
            shortcuts
                .iter()
                .map(|name| Item::plain(Marker::None, name.as_normalized()))
                .collect(),
        )
    })
}

/// The report describing the current state of one environment
fn state_report(environment: &GlobalEnvironment) -> EnvReport {
    let mut rows = Vec::new();
    rows.extend(dependencies_row(environment));
    rows.push(exposed_row(&environment.exposed));
    rows.extend(shortcuts_row(&environment.shortcuts));

    EnvReport::new(environment.name.as_str(), environment.version.clone(), None).with_rows(rows)
}

/// Print package and binaries in global environment
fn print_specific_global_environment(
    environment: &GlobalEnvironment,
    sort_by_size: bool,
    regex: Option<String>,
) -> miette::Result<()> {
    let mut report = state_report(environment);

    if !environment.channels.is_empty() {
        report.rows.push(Row::new(
            Label::Channels,
            environment
                .channels
                .iter()
                .map(|channel| Item::plain(Marker::None, channel.to_string()))
                .collect(),
        ));
    }

    if let Some(platform) = environment.platform {
        report.rows.push(Row::new(
            Label::Platform,
            vec![Item::plain(Marker::None, platform.to_string())],
        ));
    }

    // Last, so it sits against the table it adds up: every package of the
    // environment, the ones its manifest names included.
    let size: u64 = environment
        .records
        .iter()
        .filter_map(|record| record.repodata_record.package_record.size)
        .sum();
    if size > 0 {
        report.rows.push(Row::new(
            Label::Size,
            vec![Item::summary(human_bytes(size as f64))],
        ));
    }

    write_stdout(&report::render(
        &report,
        &report::RenderOptions::for_stdout(),
    ))?;
    write_stdout("")?;

    let mut packages_to_output = environment
        .records
        .iter()
        .map(|record| {
            PackageToOutput::new(
                &record.repodata_record.package_record,
                environment
                    .dependencies
                    .iter()
                    .any(|dep| dep.name == record.repodata_record.package_record.name),
            )
        })
        .collect_vec();

    // Filter according to the regex
    if let Some(ref regex) = regex {
        let regex = regex::Regex::new(regex).into_diagnostic()?;
        packages_to_output.retain(|package| regex.is_match(package.name.as_normalized()));
    }

    // Sort according to the sorting strategy
    if sort_by_size {
        packages_to_output.sort_by_key(|a| a.size_bytes.unwrap_or(0));
    } else {
        packages_to_output.sort_by(|a, b| a.name.cmp(&b.name));
    }

    print_package_table(packages_to_output).into_diagnostic()?;

    Ok(())
}

/// Print all environments in the global environment
fn print_all_global_environments(environments: &[GlobalEnvironment]) -> miette::Result<()> {
    if environments.is_empty() {
        return write_stdout("No global environments found.");
    }

    let reports = environments.iter().map(state_report).collect_vec();

    write_stdout(&report::render_all(
        &reports,
        &report::RenderOptions::for_stdout(),
    ))
}
