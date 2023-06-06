use serde::Deserialize;
use serde_with::serde_as;

/// Represents different types of scripts
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Command {
    Plain(String),
    Process(ProcessCmd),
    Alias(AliasCmd),
}

/// A command script executes a single command from the environment
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct ProcessCmd {
    // A list of arguments, the first argument denotes the command to run. When deserializing both
    // an array of strings and a single string are supported.
    pub cmd: CmdArgs,

    /// A list of commands that should be run before this one
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CmdArgs {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AliasCmd {
    /// A list of commands that should be run before this one
    pub depends_on: Vec<String>,
}
