use miette::IntoDiagnostic;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, Debug, Clone)]
pub struct CondaEnvFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    channels: Vec<String>,
    dependencies: Vec<CondaEnvDep>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum CondaEnvDep {
    Conda(String),
    Pip { pip: Vec<String> },
}

impl CondaEnvFile {
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn channels(&self) -> &Vec<String> {
        &self.channels
    }

    pub fn dependencies(&self) -> &Vec<CondaEnvDep> {
        &self.dependencies
    }

    pub fn from_path(path: &Path) -> miette::Result<Self> {
        let file = std::fs::File::open(path).into_diagnostic()?;
        let reader = std::io::BufReader::new(file);
        let env_file = serde_yaml::from_reader(reader).into_diagnostic()?;
        Ok(env_file)
    }
}
