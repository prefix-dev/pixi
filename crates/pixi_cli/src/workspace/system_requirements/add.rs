use crate::workspace::system_requirements::SystemRequirementEnum;
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_core::Workspace;
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

pub async fn execute(workspace: Workspace, args: Args) -> miette::Result<()> {
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
        .map_or_else(FeatureName::default, FeatureName::from);

    // Add the platforms to the lock-file
    let mut workspace = workspace.modify()?;
    workspace
        .manifest()
        .add_system_requirement(requirement, &feature_name)?;

    // Save the workspace to disk
    workspace.save().await.into_diagnostic()?;

    Ok(())
}
