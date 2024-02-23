use miette::IntoDiagnostic;
use serde::Deserialize;
use std::{io::BufRead, path::Path};

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

        let lines = reader
            .lines()
            .collect::<Result<Vec<String>, _>>()
            .into_diagnostic()?;
        let mut s = String::new();
        for line in lines {
            if line.contains("- sel(") {
                tracing::warn!("Skipping micromamba sel(...) in line: \"{}\"", line.trim());
                tracing::warn!("Please add the dependencies manually");
                continue;
            }
            s.push_str(&line);
            s.push('\n');
        }

        let env_file = serde_yaml::from_str(&s).into_diagnostic()?;
        Ok(env_file)
    }
}
