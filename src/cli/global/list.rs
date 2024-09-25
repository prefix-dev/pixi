use crate::global;
use crate::global::{ExposedName, Project};
use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::Itertools;
use pixi_config::{Config, ConfigCli};
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;

/// Lists all packages previously installed into a globally accessible location via `pixi global install`.
#[derive(Parser, Debug)]
pub struct Args {
    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,
    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project = global::Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());
    global::sync(&project, &config).await?;

    list_global_environments(project).await?;

    Ok(())
}

async fn list_global_environments(project: Project) -> miette::Result<()> {
    let envs = project.environments();

    let mut message = String::new();

    let len = envs.len();
    for (idx, env) in envs.into_iter().enumerate() {
        let last = (idx + 1) == len;

        if last {
            message.push_str("└──");
        } else {
            message.push_str("├──");
        }

        message.push_str(&format!(" {}", env.0.fancy_display()));

        // Write dependencies
        if let Some(dep_message) = format_dependencies(
            env.0.as_str(),
            &env.1.dependencies,
            last,
            !env.1.exposed.is_empty(),
        ) {
            message.push_str(&dep_message);
        }

        // Write exposed binaries
        if let Some(exp_message) = format_exposed(env.0.as_str(), &env.1.exposed, last) {
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
fn display_dependency(name: &PackageName, spec: &PixiSpec) -> String {
    let version = if spec.has_version_spec() {
        format!(" {}", spec.as_version_spec().expect("version spec is set"))
    } else {
        String::new()
    };

    format!(
        "{}{}",
        console::style(name.as_normalized()).green(),
        console::style(version).blue()
    )
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
    last: bool,
    more: bool,
) -> Option<String> {
    if dependencies
        .iter()
        .any(|(pkg_name, _spec)| pkg_name.as_normalized() != env_name)
    {
        let content = dependencies
            .iter()
            .map(|(name, spec)| display_dependency(name, spec))
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
