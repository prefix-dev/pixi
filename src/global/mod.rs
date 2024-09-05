// TODO: remove this before merging to main
#![allow(unused)]

mod common;
mod install;
mod project;

pub(crate) use common::{
    channel_name_from_prefix, find_designated_package, BinDir, EnvDir, EnvRoot,
};
pub(crate) use install::sync;
pub(crate) use install::{create_executable_scripts, script_exec_mapping};
pub(crate) use project::{EnvironmentName, ExposedKey, Project, MANIFEST_DEFAULT_NAME};
