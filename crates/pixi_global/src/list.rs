use std::io::Write;

use fancy_display::FancyDisplay;
use human_bytes::human_bytes;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::{IntoDiagnostic, miette};
use pixi_consts::consts;
use pixi_core::environment::list::{PackageToOutput, print_package_table};
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, PrefixRecord};
use serde::Serialize;

use super::{
    EnvironmentName, Mapping, Project,
    project::ParsedEnvironment,
    report::{self, EnvReport, Item, Label, Marker, Row},
};
use crate::common::{self, find_package_records};

/// JSON-serializable representation of an exposed mapping.
#[derive(Serialize)]
struct ExposedMappingJson {
    exposed_name: String,
    executable: String,
}

/// JSON-serializable representation of a dependency with its installed version.
#[derive(Serialize)]
struct DependencyJson {
    name: String,
    version: Option<String>,
}

/// JSON-serializable representation of a global environment.
#[derive(Serialize)]
struct GlobalEnvironmentJson {
    name: String,
    dependencies: Vec<DependencyJson>,
    exposed: Vec<ExposedMappingJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
}

/// Print global environments as JSON to stdout.
pub async fn list_global_environments_json(
    project: &Project,
    environment: Option<&EnvironmentName>,
    regex: Option<String>,
) -> miette::Result<()> {
    let mut project_envs = project.environments().clone();
    project_envs.sort_by(|a, _, b, _| a.to_string().cmp(&b.to_string()));

    project_envs.retain(|_, parsed_environment| !parsed_environment.dependencies.specs.is_empty());

    if let Some(env_name) = environment {
        project_envs.retain(|name, _| name == env_name);
        if project_envs.is_empty() {
            return Err(miette!(
                "environment {} not found",
                env_name.fancy_display()
            ));
        }
    }

    if let Some(regex) = regex {
        let regex = regex::Regex::new(&regex).into_diagnostic()?;
        project_envs.retain(|env_name, _| regex.is_match(env_name.as_str()));
    }

    let mut environments = Vec::new();
    for (env_name, env) in project_envs.iter() {
        let env_dir = project.env_root.path().join(env_name.as_str());
        let conda_meta = env_dir.join(consts::CONDA_META_DIR);
        let records = find_package_records(&conda_meta).await?;

        let dependencies = env
            .dependencies
            .specs
            .iter()
            .map(|(name, _spec)| {
                let version = records
                    .iter()
                    .find(|rec| {
                        rec.repodata_record.package_record.name.as_normalized()
                            == name.as_normalized()
                    })
                    .map(|rec| {
                        rec.repodata_record
                            .package_record
                            .version
                            .version()
                            .to_string()
                    });
                DependencyJson {
                    name: name.as_normalized().to_string(),
                    version,
                }
            })
            .collect();

        let exposed = env
            .exposed
            .iter()
            .map(|mapping| ExposedMappingJson {
                exposed_name: mapping.exposed_name().to_string(),
                executable: mapping.executable_relname().to_string(),
            })
            .collect();

        environments.push(GlobalEnvironmentJson {
            name: env_name.as_str().to_string(),
            dependencies,
            exposed,
            platform: env.platform.map(|p| p.to_string()),
        });
    }

    let json_string =
        serde_json::to_string_pretty(&environments).expect("cannot serialize environments to JSON");
    writeln!(std::io::stdout(), "{json_string}")
        .inspect_err(|e| {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        })
        .into_diagnostic()?;

    Ok(())
}

/// Write a line to stdout, treating a closed pipe as a normal end of output.
fn write_stdout(line: &str) -> miette::Result<()> {
    writeln!(std::io::stdout(), "{line}")
        .inspect_err(|e| {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        })
        .into_diagnostic()
}

fn format_mapping(mapping: &Mapping) -> Item {
    let exposed = mapping.exposed_name().to_string();
    let detail = (exposed != mapping.executable_relname())
        .then(|| format!("-> {}", mapping.executable_relname()));
    Item::exposed(Marker::None, exposed, detail)
}

/// The row listing what an environment exposes, or that it exposes nothing.
fn exposed_row(exposed: &IndexSet<Mapping>) -> Row {
    if exposed.is_empty() {
        Row::new(Label::Exposed, vec![Item::summary("nothing")])
    } else {
        Row::new(Label::Exposed, exposed.iter().map(format_mapping).collect())
    }
}

/// The row naming the dependencies of an environment with their installed
/// versions. An environment holding a single dependency named after itself
/// carries it in the header instead.
fn dependencies_row(
    env_name: &EnvironmentName,
    dependencies: &IndexMap<PackageName, PixiSpec>,
    records: &[PrefixRecord],
) -> Option<Row> {
    if !dependencies
        .keys()
        .any(|name| name.as_normalized() != env_name.as_str())
    {
        return None;
    }

    let items = dependencies
        .keys()
        .sorted_by_key(|name| name.as_normalized())
        .map(|name| {
            let version = records
                .iter()
                .find(|record| {
                    record.repodata_record.package_record.name.as_normalized()
                        == name.as_normalized()
                })
                .map(|record| {
                    record
                        .repodata_record
                        .package_record
                        .version
                        .version()
                        .to_string()
                });
            Item::package(Marker::None, name.as_normalized(), version)
        })
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

/// The report describing the current state of one environment.
async fn state_report(
    project: &Project,
    env_name: &EnvironmentName,
    environment: &ParsedEnvironment,
) -> miette::Result<EnvReport> {
    let conda_meta = project
        .env_root
        .path()
        .join(env_name.as_str())
        .join(consts::CONDA_META_DIR);
    let records = find_package_records(&conda_meta).await?;

    let version = if common::is_single_package_environment(project, env_name) {
        common::installed_version(project, env_name).await?
    } else {
        None
    };

    let mut rows = Vec::new();
    rows.extend(dependencies_row(
        env_name,
        &environment.dependencies.specs,
        &records,
    ));
    rows.push(exposed_row(&environment.exposed));
    rows.extend(shortcuts_row(
        environment.shortcuts.as_ref().unwrap_or(&IndexSet::new()),
    ));

    Ok(EnvReport::new(env_name.as_str(), version, None).with_rows(rows))
}

/// List package and binaries in global environment
pub async fn list_specific_global_environment(
    project: &Project,
    environment_name: &EnvironmentName,
    sort_by_size: bool,
    regex: Option<String>,
) -> miette::Result<()> {
    let env = project
        .environments()
        .get(environment_name)
        .ok_or_else(|| miette!("Environment {} not found", environment_name.fancy_display()))?;

    let records = find_package_records(
        &project
            .env_root
            .path()
            .join(environment_name.as_str())
            .join(consts::CONDA_META_DIR),
    )
    .await?;

    let mut report = state_report(project, environment_name, env).await?;

    if !env.channels().is_empty() {
        report.rows.push(Row::new(
            Label::Channels,
            env.channels()
                .iter()
                .map(|channel| Item::plain(Marker::None, channel.to_string()))
                .collect(),
        ));
    }

    if let Some(platform) = env.platform {
        report.rows.push(Row::new(
            Label::Platform,
            vec![Item::plain(Marker::None, platform.to_string())],
        ));
    }

    // Last, so it sits against the table it adds up: every package of the
    // environment, the ones its manifest names included.
    let size: u64 = records
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

    let mut packages_to_output = records
        .iter()
        .map(|record| {
            PackageToOutput::new(
                &record.repodata_record.package_record,
                env.dependencies
                    .specs
                    .contains_key(&record.repodata_record.package_record.name),
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

/// List all environments in the global environment
pub async fn list_all_global_environments(
    project: &Project,
    regex: Option<String>,
) -> miette::Result<()> {
    let mut project_envs = project.environments().clone();
    project_envs.sort_by(|a, _, b, _| a.to_string().cmp(&b.to_string()));

    project_envs.retain(|env_name, parsed_environment| {
        if parsed_environment.dependencies.specs.is_empty() {
            tracing::warn!(
                "Environment {} doesn't contain dependencies. Skipping.",
                env_name.fancy_display()
            );
            false
        } else {
            true
        }
    });

    if let Some(regex) = regex {
        let regex = regex::Regex::new(&regex).into_diagnostic()?;
        project_envs.retain(|env_name, _| regex.is_match(env_name.as_str()));
    }

    if project_envs.is_empty() {
        return write_stdout("No global environments found.");
    }

    let mut reports = Vec::with_capacity(project_envs.len());
    for (env_name, environment) in project_envs.iter() {
        reports.push(state_report(project, env_name, environment).await?);
    }

    write_stdout(&report::render_all(
        &reports,
        &report::RenderOptions::for_stdout(),
    ))
}
