use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};

use miette::IntoDiagnostic;
use once_cell::sync::Lazy;
use pixi_utils::executable_from_path;
use regex::Regex;
use serde::{Deserialize, Serialize};
// use tokio::fs::File;
use std::fs::File;

use fs_err::tokio as tokio_fs;

use super::ExposedName;

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "macos")]
pub const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../crates/pixi_trampoline/trampolines/pixi-trampoline-aarch64-apple-darwin");

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "windows")]
pub const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-aarch64-pc-windows-msvc.exe"
);

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "linux")]
pub const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-aarch64-unknown-linux-musl"
);

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "macos")]
pub const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-apple-darwin");

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "windows")]
pub const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-pc-windows-msvc.exe"
);

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "linux")]
pub const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-unknown-linux-musl"
);

// return file name of the executable
pub(crate) fn file_name(exposed_name: &ExposedName) -> String {
    if cfg!(target_os = "windows") {
        format!("{}.exe", exposed_name)
    } else {
        exposed_name.to_string()
    }
}

/// Extracts the executable path from a script file.
///
/// This function reads the content of the script file and attempts to extract
/// the path of the executable it references. It is used to determine
/// the actual binary path from a wrapper script.
pub(crate) async fn extract_executable_from_script(script: &Path) -> miette::Result<PathBuf> {
    // Read the script file into a string
    let script_content = tokio_fs::read_to_string(script).await.into_diagnostic()?;

    // Compile the regex pattern
    #[cfg(unix)]
    const PATTERN: &str = r#""([^"]+)" "\$@""#;
    // The pattern includes `"?` to also find old pixi global installations.
    #[cfg(windows)]
    const PATTERN: &str = r#"@"?([^"]+)"? %/*"#;
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(PATTERN).expect("Failed to compile regex"));

    // Apply the regex to the script content
    if let Some(caps) = RE.captures(&script_content) {
        if let Some(matched) = caps.get(1) {
            return Ok(PathBuf::from(matched.as_str()));
        }
    }
    tracing::debug!(
        "Failed to extract executable path from script {}",
        script_content
    );

    // Return an error if the executable path couldn't be extracted
    miette::bail!(
        "Failed to extract executable path from script {}",
        script.display()
    )
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct ManifestMetadata {
    pub exe: PathBuf,
    pub path: PathBuf,
    pub env: HashMap<String, String>,
}

impl ManifestMetadata {
    pub fn new(exe: PathBuf, path: PathBuf, env: Option<HashMap<String, String>>) -> Self {
        ManifestMetadata {
            exe,
            path,
            env: env.unwrap_or_default(),
        }
    }

    pub fn from_root_path(root_path: PathBuf, exposed_name: &ExposedName) -> Self {
        let manifest_path = root_path.join(exposed_name.to_string() + ".json");
        eprintln!("manifest_path: {:?}", manifest_path);
        let reader_file =
            std::fs::File::open(&manifest_path).expect("should be able to open manifest file");
        serde_json::from_reader(reader_file).unwrap()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalBin {
    Trampoline(Trampoline),
    Script(PathBuf),
}

impl GlobalBin {
    pub async fn executable(&self) -> miette::Result<PathBuf> {
        Ok(match self {
            GlobalBin::Trampoline(trampoline) => trampoline.original_exe(),
            GlobalBin::Script(script) => extract_executable_from_script(script).await?,
        })
    }

    pub fn exposed_name(&self) -> ExposedName {
        match self {
            GlobalBin::Trampoline(trampoline) => trampoline.exposed_name.clone(),
            GlobalBin::Script(script) => {
                ExposedName::from_str(&executable_from_path(script)).unwrap()
            }
        }
    }

    pub fn path(&self) -> PathBuf {
        match self {
            GlobalBin::Trampoline(trampoline) => trampoline.path(),
            GlobalBin::Script(script) => script.clone(),
        }
    }

    pub fn is_trampoline(&self) -> bool {
        matches!(self, GlobalBin::Trampoline(_))
    }

    pub fn trampoline(&self) -> Option<&Trampoline> {
        match self {
            GlobalBin::Trampoline(trampoline) => Some(trampoline),
            _ => None,
        }
    }

    pub fn remove(&self) {
        match self {
            GlobalBin::Trampoline(trampoline) => {
                let _ = std::fs::remove_file(trampoline.path());
                let _ = std::fs::remove_file(trampoline.manifest_path());
            }
            GlobalBin::Script(script) => {
                let _ = std::fs::remove_file(script);
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trampoline {
    // Exposed name of the trampoline
    exposed_name: ExposedName,
    // Root path where the trampoline is stored
    root_path: PathBuf,
    // Metadata of the trampoline
    metadata: ManifestMetadata,
}

impl Trampoline {
    pub fn new(exposed_name: ExposedName, root_path: PathBuf, metadata: ManifestMetadata) -> Self {
        Trampoline {
            exposed_name,
            root_path,
            metadata,
        }
    }

    pub fn from(trampoline_path: PathBuf) -> Self {
        let exposed_name = ExposedName::from_str(&executable_from_path(&trampoline_path))
            .expect("should have a valid exposed name");
        let parent_path = trampoline_path
            .parent()
            .expect("trampoline should have a parent path")
            .to_path_buf();

        let metadata = ManifestMetadata::from_root_path(parent_path.clone(), &exposed_name);

        Trampoline::new(exposed_name, parent_path, metadata)
    }

    // return the path to the trampoline
    pub fn path(&self) -> PathBuf {
        self.root_path.join(file_name(&self.exposed_name))
    }

    pub fn original_exe(&self) -> PathBuf {
        self.metadata.exe.clone()
    }

    // return the path to the trampoline manifest
    pub fn manifest_path(&self) -> PathBuf {
        self.root_path.join(self.exposed_name.to_string() + ".json")
    }

    pub async fn save(&self) -> miette::Result<()> {
        self.write_trampoline().await?;
        self.write_manifest()?;
        Ok(())
    }

    async fn write_trampoline(&self) -> miette::Result<()> {
        tokio::fs::write(self.path(), TRAMPOLINE_BIN)
            .await
            .into_diagnostic()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(self.path(), std::fs::Permissions::from_mode(0o755))
                .into_diagnostic()?;
        }

        Ok(())
    }

    fn write_manifest(&self) -> miette::Result<()> {
        let manifest_file = File::create(self.manifest_path()).into_diagnostic()?;
        serde_json::to_writer_pretty(manifest_file, &self.metadata).into_diagnostic()?;

        Ok(())
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn test_trampoline_creation() {
//         let trampoline = Trampoline::new();
//         assert!(
//             trampoline.get_binary_size() > 0,
//             "Binary should not be empty"
//         );
//     }
// }
