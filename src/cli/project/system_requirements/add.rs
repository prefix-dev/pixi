use crate::Project;
use clap::{Parser, ValueEnum};
use miette::{Context, IntoDiagnostic};
use pixi_manifest::{FeatureName, LibCFamilyAndVersion, LibCSystemRequirement, SystemRequirements};

/// Enum for valid system requirement names.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SystemRequirementEnum {
    Linux,
    Cuda,
    Macos,
    Glibc,
    Libc,
    ArchSpec,
}

#[derive(Parser, Debug)]
pub struct Args {
    /// The name of the system requirement to add.
    pub requirement: SystemRequirementEnum,

    /// The version of the requirement
    pub version: String,

    /// The name of the feature to modify.
    #[clap(long, short)]
    pub feature: Option<String>,
}

pub async fn execute(mut project: Project, args: Args) -> miette::Result<()> {
    let requirement = match args.requirement {
        SystemRequirementEnum::Linux => SystemRequirements {
            linux: Some(args.version.parse().into_diagnostic()?),
            ..Default::default()
        },
        SystemRequirementEnum::Cuda => SystemRequirements {
            cuda: Some(args.version.parse().into_diagnostic()?),
            ..Default::default()
        },
        SystemRequirementEnum::Macos => SystemRequirements {
            macos: Some(args.version.parse().into_diagnostic()?),
            ..Default::default()
        },
        SystemRequirementEnum::Glibc => SystemRequirements {
            libc: Some(LibCSystemRequirement::GlibC(
                args.version.parse().into_diagnostic()?,
            )),
            ..Default::default()
        },
        SystemRequirementEnum::Libc => {
            if let Some((version, family)) = args.version.split_once(' ') {
                let version = version.parse().into_diagnostic().wrap_err(
                    "Invalid version string, expected format: <version> <family>, e.g. '2.17 libc'",
                )?;
                SystemRequirements {
                    libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                        family: Some(family.to_string()),
                        version,
                    })),
                    ..Default::default()
                }
            } else {
                SystemRequirements {
                    libc: Some(LibCSystemRequirement::GlibC(
                        args.version.parse().into_diagnostic()?,
                    )),
                    ..Default::default()
                }
            }
        }
        SystemRequirementEnum::ArchSpec => SystemRequirements {
            archspec: Some(args.version.parse().into_diagnostic()?),
            ..Default::default()
        },
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
