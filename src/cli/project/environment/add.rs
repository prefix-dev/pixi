use clap::Parser;

use crate::{EnvironmentName, Project};

#[derive(Parser, Debug)]
pub struct Args {
    /// The environment name(s) to add.
    pub name: String,

    /// Features to add to the environment.
    pub features: Option<Vec<String>>,

    /// The solve-group to add the environment to.
    #[clap(long)]
    pub solve_group: Option<String>,

    /// Whether or not to include properties from the default feature.
    #[clap(default_value = "false", long)]
    pub no_default_feature: bool,

    /// Whether to update the manifest even if the environment already exists.
    #[clap(default_value = "false", long)]
    pub force: bool,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    let environment_exists = project
        .environment(&EnvironmentName::Named(args.name.clone()))
        .is_some();
    if environment_exists && !args.force {
        return Err(miette::miette!(
            help = "use --force to overwrite the existing environment",
            "the environment '{}' already exists",
            args.name
        ));
    }

    // Add the platforms to the lock-file
    project.manifest.add_environment(
        args.name.clone(),
        args.features,
        args.solve_group,
        args.no_default_feature,
    )?;

    // Save the project to disk
    project.save()?;

    // Report back to the user
    eprintln!(
        "{}{} environment {}",
        console::style(console::Emoji("✔ ", "")).green(),
        if environment_exists {
            "Updated"
        } else {
            "Added"
        },
        args.name
    );

    Ok(())
}
