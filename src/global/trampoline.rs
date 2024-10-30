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

use fs_err::tokio as tokio_fs;
use tokio::io::AsyncReadExt;

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

/// Returns the file name of the executable
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

/// Manifest data of the original executable.
/// This is used by trampoline to set the environment variables
/// prepened the path and execute the original executable.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct ManifestMetadata {
    /// Path to the original executable.
    pub exe: PathBuf,
    /// Root path of the original executable that should be prepended to the PATH.
    pub path: PathBuf,
    /// Environment variables to be set before executing the original executable.
    pub env: HashMap<String, String>,
}

impl ManifestMetadata {
    /// Create a new manifest metadata.
    pub fn new(exe: PathBuf, path: PathBuf, env: Option<HashMap<String, String>>) -> Self {
        ManifestMetadata {
            exe,
            path,
            env: env.unwrap_or_default(),
        }
    }

    /// Read existing manifest metadata from the root path.
    pub async fn from_root_path(
        root_path: PathBuf,
        exposed_name: &ExposedName,
    ) -> miette::Result<Self> {
        let manifest_path = root_path.join(exposed_name.to_string() + ".json");
        let manifest_str = tokio_fs::read_to_string(manifest_path)
            .await
            .into_diagnostic()?;
        serde_json::from_str(&manifest_str).into_diagnostic()
    }
}

/// Represents an exposed global executable installed by pixi global.
/// This can be either a trampoline or a old script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalBin {
    Trampoline(Trampoline),
    Script(PathBuf),
}

impl GlobalBin {
    /// Returns the path to the original executable.
    pub async fn executable(&self) -> miette::Result<PathBuf> {
        Ok(match self {
            GlobalBin::Trampoline(trampoline) => trampoline.original_exe(),
            GlobalBin::Script(script) => extract_executable_from_script(script).await?,
        })
    }

    /// Returns exposed name
    pub fn exposed_name(&self) -> ExposedName {
        match self {
            GlobalBin::Trampoline(trampoline) => trampoline.exposed_name.clone(),
            GlobalBin::Script(script) => {
                ExposedName::from_str(&executable_from_path(script)).unwrap()
            }
        }
    }

    /// Returns the path to the exposed binary.
    pub fn path(&self) -> PathBuf {
        match self {
            GlobalBin::Trampoline(trampoline) => trampoline.path(),
            GlobalBin::Script(script) => script.clone(),
        }
    }

    /// Returns if the exposed global binary is trampoline.
    pub fn is_trampoline(&self) -> bool {
        matches!(self, GlobalBin::Trampoline(_))
    }

    /// Returns the inner trampoline.
    pub fn trampoline(&self) -> Option<&Trampoline> {
        match self {
            GlobalBin::Trampoline(trampoline) => Some(trampoline),
            _ => None,
        }
    }

    /// Removes exposed global executable.
    /// In case it is a trampoline, it will also remove its manifest.
    pub async fn remove(&self) -> miette::Result<()> {
        match self {
            GlobalBin::Trampoline(trampoline) => {
                let (trampoline_removed, manifest_removed) = tokio::join!(
                    tokio_fs::remove_file(trampoline.path()),
                    tokio_fs::remove_file(trampoline.manifest_path())
                );
                trampoline_removed.into_diagnostic()?;
                manifest_removed.into_diagnostic()?;
            }
            GlobalBin::Script(script) => {
                tokio_fs::remove_file(script).await.into_diagnostic()?;
            }
        }

        Ok(())
    }
}

/// Represents a trampoline binary installed by pixi.
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
    /// Creates a new trampoline.
    pub fn new(exposed_name: ExposedName, root_path: PathBuf, metadata: ManifestMetadata) -> Self {
        Trampoline {
            exposed_name,
            root_path,
            metadata,
        }
    }

    /// Tries to create a trampoline object from the already existing trampoline.
    pub async fn try_from(trampoline_path: PathBuf) -> miette::Result<Self> {
        let exposed_name = ExposedName::from_str(&executable_from_path(&trampoline_path))?;
        let parent_path = trampoline_path
            .parent()
            .ok_or_else(|| {
                miette::miette!(
                    "trampoline {} should have a parent path",
                    trampoline_path.display()
                )
            })?
            .to_path_buf();

        let metadata = ManifestMetadata::from_root_path(parent_path.clone(), &exposed_name).await?;

        Ok(Trampoline::new(exposed_name, parent_path, metadata))
    }

    /// Returns the path to the trampoline
    pub fn path(&self) -> PathBuf {
        self.root_path.join(file_name(&self.exposed_name))
    }

    pub fn original_exe(&self) -> PathBuf {
        self.metadata.exe.clone()
    }

    /// Returns the path to the trampoline manifest
    pub fn manifest_path(&self) -> PathBuf {
        self.root_path.join(self.exposed_name.to_string() + ".json")
    }

    pub async fn save(&self) -> miette::Result<()> {
        let (trampoline, manifest) = tokio::join!(self.write_trampoline(), self.write_manifest());
        trampoline?;
        manifest?;
        Ok(())
    }

    async fn write_trampoline(&self) -> miette::Result<()> {
        tokio_fs::write(self.path(), TRAMPOLINE_BIN)
            .await
            .into_diagnostic()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio_fs::set_permissions(self.path(), std::fs::Permissions::from_mode(0o755))
                .await
                .into_diagnostic()?;
        }

        Ok(())
    }

    /// Writes the manifest file of the trampoline
    async fn write_manifest(&self) -> miette::Result<()> {
        let manifest_string = serde_json::to_string_pretty(&self.metadata).into_diagnostic()?;
        tokio_fs::write(self.manifest_path(), manifest_string)
            .await
            .into_diagnostic()?;

        Ok(())
    }

    /// Checks if executable is a saved trampoline
    /// by reading only first 1048 bytes of the file
    pub async fn is_trampoline(path: &Path) -> miette::Result<bool> {
        let mut bin_file = tokio_fs::File::open(path).await.into_diagnostic()?;

        let mut buf = [0; 1048];
        bin_file.read_exact(buf.as_mut()).await.into_diagnostic()?;
        Ok(buf == TRAMPOLINE_BIN[..1048])
    }
}
