use std::{fmt::Display, path::PathBuf};

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_virtual_packages::VirtualPackage;
use serde::Serialize;
use serde_with::serde_as;
use serde_with::DisplayFromStr;

use crate::Project;

#[derive(Parser, Debug)]
pub struct Args {
    /// Wether to show the output as JSON or not
    #[arg(long)]
    json: bool,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

#[derive(Serialize)]
pub struct ProjectInfo {
    tasks: Vec<String>,
    manifest_path: PathBuf,
}

#[serde_as]
#[derive(Serialize)]
pub struct Info {
    platform: String,
    #[serde_as(as = "Vec<DisplayFromStr>")]
    virtual_packages: Vec<GenericVirtualPackage>,
    version: String,
    cache_dir: Option<PathBuf>,
    project_info: Option<ProjectInfo>,
}

impl Display for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cache_dir = match &self.cache_dir {
            Some(path) => path.to_string_lossy().to_string(),
            None => "None".to_string(),
        };

        writeln!(f, "pixi {}\n", self.version)?;
        writeln!(f, "{:20}: {}", "Platform", self.platform)?;

        for (i, p) in self.virtual_packages.iter().enumerate() {
            if i == 0 {
                writeln!(f, "{:20}: {}", "Virtual packages", p)?;
            } else {
                writeln!(f, "{:20}: {}", "", p)?;
            }
        }

        writeln!(f, "{:20}: {}", "Cache dir", cache_dir)?;

        if let Some(pi) = self.project_info.as_ref() {
            writeln!(f, "\nProject\n------------\n")?;

            writeln!(
                f,
                "{:20}: {}",
                "Manifest file",
                pi.manifest_path.to_string_lossy()
            )?;

            writeln!(f, "Tasks:")?;
            for c in &pi.tasks {
                writeln!(f, "  - {}", c)?;
            }
        }

        Ok(())
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref()).ok();

    let project_info = project.map(|p| ProjectInfo {
        manifest_path: p.root().to_path_buf().join("pixi.toml"),
        tasks: p.manifest.tasks.keys().cloned().collect(),
    });

    let virtual_packages = VirtualPackage::current()
        .into_diagnostic()?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .collect::<Vec<_>>();

    let info = Info {
        platform: Platform::current().to_string(),
        virtual_packages,
        version: env!("CARGO_PKG_VERSION").to_string(),
        cache_dir: rattler::default_cache_dir().ok(),
        project_info,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&info).into_diagnostic()?);
        Ok(())
    } else {
        println!("{}", info);
        Ok(())
    }
}
