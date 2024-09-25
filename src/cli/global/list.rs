use crate::global;
use crate::global::common::find_package_records;
use crate::global::{EnvironmentName, ExposedName, Project};
use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::miette;
use pixi_config::{Config, ConfigCli};
use pixi_consts::consts;
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, PrefixRecord, Version};
use std::str::FromStr;

/// Lists all packages previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
pub struct Args {
    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,
    #[clap(flatten)]
    config: ConfigCli,

    /// The name of the environment to list.
    #[clap(short, long)]
    environment: Option<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project = global::Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());
    global::sync(&project, &config).await?;

    if let Some(environment) = args.environment {
        let name = EnvironmentName::from_str(environment.as_str())?;
        list_environment(project, &name).await?;
    } else {
        list_global_environments(project).await?;
    }

    Ok(())
}

/// List package and binaries in environment
async fn list_environment(
    project: Project,
    environment_name: &EnvironmentName,
) -> miette::Result<()> {
    let _env = project
        .environments()
        .get(environment_name)
        .ok_or_else(|| miette!("Environment '{}' not found", environment_name))?;

    let records = find_package_records(
        &project
            .env_root
            .path()
            .join(environment_name.as_str())
            .join(consts::CONDA_META_DIR),
    )
    .await?;

    let mut message = String::new();

    message.push_str(&format!(
        "Environment: {}\n",
        environment_name.fancy_display()
    ));
    let len = records.len();
    for (idx, record) in records.iter().enumerate() {
        if (idx + 1) == len {
            message.push_str("└──");
        } else {
            message.push_str("├──");
        }
        message.push_str(&format!(
            " {}: {} {}\n",
            record.repodata_record.package_record.name.as_normalized(),
            console::style(record.repodata_record.package_record.version.clone()).blue(),
            record.repodata_record.package_record.build.clone(),
        ));
    }

    // Write exposed binaries
    let exposed = project
        .environments()
        .get(environment_name)
        .map(|env| env.exposed());
    if let Some(exposed) = exposed {
        if !records.is_empty() {
            message.push_str(&format!(
                "Exposes: {}",
                exposed
                    .iter()
                    .map(|(exp, from)| format!(
                        "{}{}",
                        console::style(exp).yellow().to_string(),
                        if from != &exp.to_string() {
                            format!(" from ({})", console::style(from).yellow())
                        } else {
                            "".to_string()
                        }
                    ))
                    .join(", "),
            ));
        }
    }

    println!("{}", message);

    Ok(())
}

async fn list_global_environments(project: Project) -> miette::Result<()> {
    let envs = project.environments();

    let mut message = String::new();

    let len = envs.len();
    for (idx, (env_name, env)) in envs.into_iter().enumerate() {
        let env_dir = project.env_root.path().join(env_name.as_str());
        let records = find_package_records(&env_dir.join(consts::CONDA_META_DIR)).await?;

        let last = (idx + 1) == len;

        if last {
            message.push_str("└──");
        } else {
            message.push_str("├──");
        }

        if !env
            .dependencies()
            .iter()
            .any(|(pkg_name, _spec)| pkg_name.as_normalized() != env_name.as_str())
        {
            if let Some(env_package) = records.iter().find(|rec| {
                rec.repodata_record.package_record.name.as_normalized() == env_name.as_str()
            }) {
                message.push_str(&format!(
                    " {}: {}",
                    env_name.fancy_display(),
                    console::style(env_package.repodata_record.package_record.version.clone())
                        .blue()
                ));
            } else {
                message.push_str(&format!(" {}", env_name.fancy_display()));
            }
        } else {
            message.push_str(&format!(" {}", env_name.fancy_display()));
        }

        // Write dependencies
        if let Some(dep_message) = format_dependencies(
            env_name.as_str(),
            &env.dependencies,
            &records,
            last,
            !env.exposed.is_empty(),
        ) {
            message.push_str(&dep_message);
        }

        // Write exposed binaries
        if let Some(exp_message) = format_exposed(env_name.as_str(), env.exposed(), last) {
            message.push_str(&exp_message);
        }

        if !last {
            message.push('\n');
        }
    }

    eprintln!("Global environments:\n{}", message);

    Ok(())
}

/// Display a dependency in a human-readable format.
fn display_dependency(name: &PackageName, version: Option<Version>) -> String {
    if let Some(version) = version {
        format!(
            "{} {}",
            console::style(name.as_normalized()).green(),
            console::style(version).blue()
        )
    } else {
        console::style(name.as_normalized()).green().to_string()
    }
}

/// Creating the ASCII art representation of a section.
fn format_asciiart_section(label: &str, content: String, last: bool, more: bool) -> String {
    let prefix = if last { " " } else { "│" };
    let symbol = if more { "├" } else { "└" };
    format!("\n{}   {}─ {}: {}", prefix, symbol, label, content)
}

fn format_dependencies(
    env_name: &str,
    dependencies: &IndexMap<PackageName, PixiSpec>,
    records: &[PrefixRecord],
    last: bool,
    more: bool,
) -> Option<String> {
    if dependencies
        .iter()
        .any(|(pkg_name, _spec)| pkg_name.as_normalized() != env_name)
    {
        let content = dependencies
            .iter()
            .map(|(name, _spec)| {
                let version = records
                    .iter()
                    .find(|rec| {
                        rec.repodata_record.package_record.name.as_normalized()
                            == name.as_normalized()
                    })
                    .map(|rec| rec.repodata_record.package_record.version.version().clone());
                display_dependency(name, version)
            })
            .join(", ");
        Some(format_asciiart_section("dependencies", content, last, more))
    } else {
        None
    }
}

fn format_exposed(
    env_name: &str,
    exposed: &IndexMap<ExposedName, String>,
    last: bool,
) -> Option<String> {
    if exposed.iter().any(|(exp, _)| exp.to_string() != env_name) {
        let content = exposed
            .iter()
            .map(|(exp, _)| console::style(exp).yellow().to_string())
            .join(", ");
        Some(format_asciiart_section("exposes", content, last, false))
    } else {
        None
    }
}
