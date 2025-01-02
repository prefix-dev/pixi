use crate::Project;
use clap::{Parser, ValueEnum};
use miette::IntoDiagnostic;
use pixi_manifest::{FeatureName, LibCFamilyAndVersion, LibCSystemRequirement, SystemRequirements};

/// Enum for valid system requirement names.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SystemRequirementEnum {
    /// The version of the linux kernel (Find with `uname -r`)
    Linux,
    /// The version of the CUDA driver (Find with `nvidia-smi`)
    Cuda,
    /// The version of MacOS (Find with `sw_vers`)
    Macos,
    /// The version of the glibc library (Find with `ldd --version`)
    Glibc,
    /// Non Glibc libc family and version (Find with `ldd --version`)
    OtherLibc,
    ArchSpec,
}

#[derive(Parser, Debug)]
pub struct Args {
    /// The name of the system requirement to add.
    pub requirement: SystemRequirementEnum,

    /// The version of the requirement
    pub version: String,

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
        SystemRequirementEnum::OtherLibc => {
            if let Some(family) = args.family {
                SystemRequirements {
                    libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                        family: Some(family),
                        version: args.version.parse().into_diagnostic()?,
                    })),
                    ..Default::default()
                }
            } else {
                SystemRequirements {
                    libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                        family: None,
                        version: args.version.parse().into_diagnostic()?,
                    })),
                    ..Default::default()
                }
            }
        }
        SystemRequirementEnum::ArchSpec => SystemRequirements {
            archspec: Some(args.version),
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
