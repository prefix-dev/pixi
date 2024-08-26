// TODO: remove this before merging to main
#![allow(unused)]

mod common;
pub(crate) mod install;
mod project;

pub(crate) use common::{
    channel_name_from_prefix, find_designated_package, BinDir, EnvDir, EnvRoot,
};
pub(crate) use project::{EnvironmentName, Project};
