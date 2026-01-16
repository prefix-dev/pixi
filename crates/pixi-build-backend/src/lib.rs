pub mod cli;
pub mod generated_recipe;
pub mod intermediate_backend;
pub mod protocol;
pub mod rattler_build_integration;
pub mod server;
pub mod specs_conversion;

pub mod cache;
pub mod common;
pub mod compilers;
pub mod dependencies;
mod encoded_source_spec_url;
pub mod source;
pub mod tools;
pub mod traits;
pub mod utils;
pub mod variants;

pub mod consts;

pub use traits::{PackageSourceSpec, PackageSpec, ProjectModel, TargetSelector, Targets};

pub use cli::main_ext as cli_main;

pub use rattler_build::NormalizedKey;
pub use rattler_build::recipe::variable::Variable;
