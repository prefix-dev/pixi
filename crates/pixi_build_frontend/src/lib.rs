//! This module provides a way to build a project using a build tool.
//! It should abstract away the build tool and command to be used
//! So that in the main code, we can just call the build function
//! and it will figure out what to instatiate and how to build the project
#![deny(missing_docs)]
mod build;
mod build_tool_info;
pub mod options;

pub use build::build;
pub use build_tool_info::BuildToolInfo;
