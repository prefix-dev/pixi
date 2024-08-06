mod resolve;

use crate::options::BuildToolSpec;
use crate::BuildToolInfo;
use std::{
    io::Stderr,
    path::{Path, PathBuf},
    process::Stdio,
};

/// Build environment to be used for building the project
/// will represent the build tool and command to be used
/// can be cached for re-usability
#[derive(Debug)]
struct BuildEnvironment {
    /// Path to the build tool
    pub build_tool_path: PathBuf,
    /// Command to execute
    pub command: Option<String>,
    /// Arguments to pass to the command
    pub args: Option<Vec<String>>,
}

/// Result of building a package
pub struct BuildResult {
    /// Outcome of the build
    pub success: bool,
}

impl BuildEnvironment {
    pub fn new(
        build_tool_path: PathBuf,
        command: Option<String>,
        args: Option<Vec<String>>,
    ) -> Self {
        Self {
            build_tool_path,
            command,
            args,
        }
    }

    /// Build the project, using the specifeid build tool and command
    // TODO: might not to make all of it async eventually
    pub fn build(&self) -> Result<BuildResult, std::io::Error> {
        let rest = self.args.iter().flatten();
        let args: Vec<_> = self.command.iter().chain(rest).cloned().collect();
        // Run the build command
        // Figure out the channel location, and matchspec of the built packages
        // and how to retrieve this
        // TODO: discuss with @bzalmstra later
        let cmd = std::process::Command::new(&self.build_tool_path)
            .args(args)
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?;

        Ok(BuildResult {
            success: cmd.status.success(),
        })
    }
}

/// Setup the build environment, this should resolve into something that can be used to build the project
fn create_build_env(info: &BuildToolInfo, work_dir: &Path) -> BuildEnvironment {
    let env = match &info.build_tool {
        BuildToolSpec::CondaPackage(_) => {
            todo!("need to implement conda package build environment setup")
        }
        // TODO: command, and canonicalize path
        BuildToolSpec::DirectBinary(path) => {
            BuildEnvironment::new(
                path.clone(),
                None,
                // Guess we need to standardize this
                // TODO: discuss with @bzalmstra later
                Some(vec![
                    "--manifest-path".to_string(),
                    work_dir
                        .join("pixi.toml")
                        .to_str()
                        .expect("could not convert path to string")
                        .to_owned(),
                ]),
            )
        }
    };
    env
}

#[derive(thiserror::Error, Debug)]
pub enum BuildError {
    #[error("Failure during build execution")]
    IOError(#[from] std::io::Error),
}

/// Build the source given the build tool information
pub fn build(info: &BuildToolInfo, work_dir: &Path) -> Result<BuildResult, BuildError> {
    let env = create_build_env(info, work_dir);
    tracing::info!("Building with {:?}", info);
    Ok(env.build()?)
}
