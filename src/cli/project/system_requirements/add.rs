use crate::cli::project::system_requirements::SystemRequirementEnum;
use crate::Project;
use clap::Parser;
use pixi_manifest::{FeatureName, LibCFamilyAndVersion, LibCSystemRequirement, SystemRequirements};

#[derive(Parser, Debug)]
pub struct Args {
    /// The name of the system requirement to add.
    pub requirement: SystemRequirementEnum,

    /// The version of the requirement
    pub version: rattler_conda_types::Version,

    /// The Libc family, this can only be specified for requirement `other-libc`
    #[clap(long, required_if_eq("requirement", "other-libc"))]
    pub family: Option<String>,

    /// The name of the feature to modify.
    #[clap(long, short)]
    pub feature: Option<String>,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    let requirement = match args.requirement {
        SystemRequirementEnum::Linux => SystemRequirements {
            linux: Some(args.version),
            ..Default::default()
        },
        SystemRequirementEnum::Cuda => SystemRequirements {
            cuda: Some(args.version),
            ..Default::default()
        },
        SystemRequirementEnum::Macos => SystemRequirements {
            macos: Some(args.version),
            ..Default::default()
        },
        SystemRequirementEnum::Glibc => SystemRequirements {
            libc: Some(LibCSystemRequirement::GlibC(args.version)),
            ..Default::default()
        },
        SystemRequirementEnum::OtherLibc => {
            if let Some(family) = args.family {
                SystemRequirements {
                    libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                        family: Some(family),
                        version: args.version,
                    })),
                    ..Default::default()
                }
            } else {
                SystemRequirements {
                    libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                        family: None,
                        version: args.version,
                    })),
                    ..Default::default()
                }
            }
        }
    };

    let feature_name = args
        .feature
        .clone()
        .map_or(FeatureName::Default, FeatureName::Named);

    // Add the platforms to the lock-file
    project
        .manifest
        .add_system_requirement(requirement, &feature_name)?;

    // Save the project to disk
    project.save()?;

    Ok(())
}
