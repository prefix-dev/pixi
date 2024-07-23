use std::{collections::HashMap, str::FromStr};

use clap::Parser;
use indexmap::IndexMap;
use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness, Platform};
/// Serde structs to read global install manifest file
use serde::{Deserialize, Serialize};

use crate::config::{home_path, Config};

use super::{
    common::{get_client_and_sparse_repodata, load_package_records, BinEnvDir},
    install::{globally_install_packages, BinarySelector},
};

#[derive(Serialize, Deserialize)]
pub struct GlobalDependency {
    pub spec: Option<String>,
    pub expose_binaries: Option<HashMap<String, String>>,
}

#[derive(Serialize, Deserialize)]
pub struct GlobalEnv {
    pub dependencies: IndexMap<PackageName, GlobalDependency>,
}

impl GlobalEnv {
    /// Get all the matchspecs from the environment
    pub fn specs(&self) -> Vec<MatchSpec> {
        let mut match_specs = vec![];
        for (name, dep) in &self.dependencies {
            let spec = dep.spec.clone().unwrap_or("*".to_string());
            println!("Spec: {} {}", name.as_normalized(), spec);
            let match_spec = MatchSpec::from_str(
                &format!("{} {}", name.as_normalized(), spec),
                ParseStrictness::Lenient,
            )
            .unwrap();
            match_specs.push(match_spec);
        }
        match_specs
    }

    pub fn specific_binaries(&self) -> HashMap<PackageName, HashMap<String, String>> {
        let mut binaries = HashMap::new();
        for (name, dep) in &self.dependencies {
            if let Some(expose_binaries) = &dep.expose_binaries {
                binaries.insert(name.clone(), expose_binaries.clone());
            }
        }
        binaries
    }
}

#[derive(Serialize, Deserialize)]
pub struct GlobalManifest {
    pub envs: IndexMap<String, GlobalEnv>,
}

impl GlobalManifest {
    pub fn new() -> Self {
        GlobalManifest {
            envs: IndexMap::new(),
        }
    }

    pub fn store(&self) {
        let manifest_path = home_path()
            .expect("did not find home path")
            .join("global_manifest.yaml");
        println!("Storing global manifest to {:?}", manifest_path);
        let manifest_file = std::fs::write(manifest_path, serde_yaml::to_string(&self).unwrap());
    }

    pub async fn setup_envs(&self) -> miette::Result<()> {
        // Figure out what channels we are using
        let config = Config::load_global();
        let channels = config
            .compute_channels(&["conda-forge".to_string()])
            .unwrap();

        let (authenticated_client, sparse_repodata) =
            get_client_and_sparse_repodata(&channels, Platform::current(), &config).await?;

        for (name, env) in &self.envs {
            let env_dir = home_path()
                .expect("did not find home path")
                .join(".pixi")
                .join("envs")
                .join(name);
            std::fs::create_dir_all(env_dir).unwrap();
            let specs = env.specs();
            let records = load_package_records(&specs, sparse_repodata.values())?;
            let names = specs
                .iter()
                .map(|s| s.name.clone().unwrap())
                .collect::<Vec<_>>();
            let env_dir = BinEnvDir::create(&PackageName::from_str(name).unwrap()).await?;

            let scripts = globally_install_packages(
                env_dir,
                &names,
                records,
                authenticated_client.clone(),
                Platform::current(),
                BinarySelector::Specific(env.specific_binaries()),
            )
            .await
            .unwrap();
        }
        Ok(())
    }
}

pub fn read_global_manifest() -> GlobalManifest {
    let manifest_path = home_path()
        .expect("did not find home path")
        .join("global_manifest.yaml");
    println!("Reading global manifest from {:?}", manifest_path);
    let manifest_file = std::fs::read_to_string(manifest_path).unwrap();
    let manifest: GlobalManifest = serde_yaml::from_str(&manifest_file).unwrap();
    manifest
}
