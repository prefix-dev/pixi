// TODO: remove this before merging to main
#![allow(unused)]

mod common;
mod project;

pub(crate) use common::{
    bin_dir, bin_env_dir, channel_name_from_prefix, find_designated_package,
    find_installed_package, BinDir, BinEnvDir,
};
pub(crate) use project::Project;
