/// We are using some optimizations to reduce the size of trampoline binaries
/// and how we store them.
///
///
/// ### Compression with Zstandard (zstd)
///
/// Trampoline files are compressed using the zstd algorithm. This approach can save around 50% of storage per trampoline
/// when it's included in pixi.
///
///
/// ### Hardlinking
///
/// Instead of copying trampolines each time when we install a global binary, we store the decompressed
/// trampoline only once in `.pixi/bin/trampoline_configuration/trampoline_bin`.
/// Later we use hardlinks to point to the
/// original file as needed, reducing redundant data duplication.
///
use std::{
    collections::HashMap,
    io::ErrorKind,
    path::{Path, PathBuf},
    str::FromStr,
    sync::LazyLock,
};

use miette::IntoDiagnostic;
use pixi_utils::executable_from_path;
use regex::Regex;
use serde::{Deserialize, Serialize};

use fs_err::tokio as tokio_fs;
use tokio::io::AsyncReadExt;

use super::ExposedName;

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "macos")]
const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../../../trampoline/binaries/pixi-trampoline-aarch64-apple-darwin.zst");

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "windows")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../../../trampoline/binaries/pixi-trampoline-aarch64-pc-windows-msvc.exe.zst"
);

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../../../trampoline/binaries/pixi-trampoline-aarch64-unknown-linux-musl.zst"
);

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "macos")]
const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../../../trampoline/binaries/pixi-trampoline-x86_64-apple-darwin.zst");

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "windows")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../../../trampoline/binaries/pixi-trampoline-x86_64-pc-windows-msvc.exe.zst"
);

#[cfg(target_arch = "powerpc64")]
#[cfg(target_endian = "little")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../../../trampoline/binaries/pixi-trampoline-powerpc64le-unknown-linux-gnu.zst"
);

#[cfg(target_arch = "riscv64")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../../../trampoline/binaries/pixi-trampoline-riscv64gc-unknown-linux-gnu.zst"
);

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../../../trampoline/binaries/pixi-trampoline-x86_64-unknown-linux-musl.zst");

#[cfg(target_arch = "loongarch64")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../../../trampoline/binaries/pixi-trampoline-loongarch64-unknown-linux-gnu.zst"
);

// trampoline configuration folder name
pub const TRAMPOLINE_CONFIGURATION: &str = "trampoline_configuration";
// original trampoline binary name
pub const TRAMPOLINE_BIN_NAME: &str = "trampoline_bin";

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
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(PATTERN).expect("Failed to compile regex"));

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

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum ConfigurationParseError {
    #[error("Failed to read configuration file at {0}")]
    #[diagnostic(code(configuration::read_error))]
    ReadError(PathBuf, #[source] std::io::Error),

    #[error("Failed to parse configuration file at {0}")]
    #[diagnostic(code(configuration::parse_error))]
    ParseError(PathBuf, #[source] serde_json::Error),
}

/// Configuration of the original executable.
/// This is used by trampoline to set the environment variables
/// prepened the path and execute the original executable.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Configuration {
    /// Path to the original executable.
    pub exe: PathBuf,
    /// Root path of the original executable that should be prepended to the PATH.
    pub path_diff: String,
    /// Environment variables to be set before executing the original executable.
    pub env: HashMap<String, String>,
}

impl Configuration {
    /// Create a new configuration of trampoline.
    pub fn new(exe: PathBuf, path_diff: String, env: HashMap<String, String>) -> Self {
        Configuration {
            exe,
            path_diff,
            env,
        }
    }

    /// Read existing configuration of trampoline from the root path.
    pub async fn from_root_path(
        root_path: &Path,
        exposed_name: &ExposedName,
    ) -> Result<Self, ConfigurationParseError> {
        let configuration_path = Self::path_from_trampoline(root_path, exposed_name);
        let manifest_str = tokio_fs::read_to_string(&configuration_path)
            .await
            .map_err(|e| ConfigurationParseError::ReadError(configuration_path.clone(), e))?;
        serde_json::from_str(&manifest_str)
            .map_err(|e| ConfigurationParseError::ParseError(configuration_path.clone(), e))
    }

    /// Return the configuration file for the trampoline.
    pub fn path_from_trampoline(root_path: &Path, exposed_name: &ExposedName) -> PathBuf {
        root_path
            .join(PathBuf::from(TRAMPOLINE_CONFIGURATION))
            .join(format!("{exposed_name}.json"))
    }
}

/// Represents an exposed global executable installed by pixi global.
/// This can be either a trampoline or a old script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalExecutable {
    Trampoline(Trampoline),
    Script(PathBuf),
}

impl GlobalExecutable {
    /// Returns the path to the original executable.
    pub async fn executable(&self) -> miette::Result<PathBuf> {
        Ok(match self {
            GlobalExecutable::Trampoline(trampoline) => trampoline.original_exe(),
            GlobalExecutable::Script(script) => extract_executable_from_script(script).await?,
        })
    }

    /// Returns exposed name
    pub fn exposed_name(&self) -> ExposedName {
        match self {
            GlobalExecutable::Trampoline(trampoline) => trampoline.exposed_name.clone(),
            GlobalExecutable::Script(script) => {
                ExposedName::from_str(&executable_from_path(script))
                    .expect("It shouldn't be possible to expose `pixi` as a script")
            }
        }
    }

    /// Returns the path to the exposed binary.
    pub fn path(&self) -> PathBuf {
        match self {
            GlobalExecutable::Trampoline(trampoline) => trampoline.path(),
            GlobalExecutable::Script(script) => script.clone(),
        }
    }

    /// Returns if the exposed global binary is trampoline.
    pub fn is_trampoline(&self) -> bool {
        matches!(self, GlobalExecutable::Trampoline(_))
    }

    /// Removes exposed global executable.
    /// In case it is a trampoline, it will also remove its manifest.
    pub async fn remove(&self) -> miette::Result<()> {
        match self {
            GlobalExecutable::Trampoline(trampoline) => {
                let (trampoline_removed, manifest_removed) = tokio::join!(
                    tokio_fs::remove_file(trampoline.path()),
                    tokio_fs::remove_file(trampoline.configuration())
                );
                trampoline_removed.into_diagnostic()?;
                manifest_removed.into_diagnostic()?;
            }
            GlobalExecutable::Script(script) => {
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
    /// Exposed name of the trampoline
    exposed_name: ExposedName,
    /// Root path where the trampoline is stored
    root_path: PathBuf,
    /// Metadata of the trampoline
    configuration: Configuration,
}

impl Trampoline {
    /// Creates a new trampoline.
    pub fn new(
        exposed_name: ExposedName,
        root_path: PathBuf,
        configuration: Configuration,
    ) -> Self {
        Trampoline {
            exposed_name,
            root_path,
            configuration,
        }
    }

    /// Tries to create a trampoline object from the already existing trampoline.
    pub async fn try_from(trampoline_path: &Path) -> miette::Result<Self> {
        let exposed_name = ExposedName::from_str(&executable_from_path(trampoline_path))?;
        let parent_path = trampoline_path
            .parent()
            .ok_or_else(|| {
                miette::miette!(
                    "trampoline {} should have a parent path",
                    trampoline_path.display()
                )
            })?
            .to_path_buf();

        let metadata = Configuration::from_root_path(&parent_path, &exposed_name).await?;

        Ok(Trampoline::new(exposed_name, parent_path, metadata))
    }

    /// Returns the path to the trampoline
    pub fn path(&self) -> PathBuf {
        self.root_path.join(file_name(&self.exposed_name))
    }

    pub fn original_exe(&self) -> PathBuf {
        self.configuration.exe.clone()
    }

    /// Returns the path to the trampoline configuration
    pub fn configuration(&self) -> PathBuf {
        self.root_path
            .join(TRAMPOLINE_CONFIGURATION)
            .join(self.exposed_name.to_string() + ".json")
    }

    /// Return the path to the original trampoline binary,
    /// from what all hardlinks are created.
    fn trampoline_path(&self) -> PathBuf {
        self.root_path
            .join(TRAMPOLINE_CONFIGURATION)
            .join(TRAMPOLINE_BIN_NAME)
    }

    /// Returns the name of the trampoline
    pub fn name(trampoline: &Path) -> miette::Result<ExposedName> {
        let trampoline_name = trampoline.file_name().ok_or_else(|| {
            miette::miette!(
                "trampoline needs to have a file name {}",
                trampoline.display()
            )
        })?;
        // strip .exe from the file name
        let exposed_name = if cfg!(windows) {
            trampoline_name
                .to_string_lossy()
                .strip_suffix(".exe")
                .ok_or_else(|| miette::miette!("Trampoline doesn't have '.exe' suffix"))?
                .to_string()
        } else {
            trampoline_name.to_string_lossy().to_string()
        };

        ExposedName::from_str(&exposed_name).into_diagnostic()
    }

    pub async fn save(&self) -> miette::Result<()> {
        let (trampoline, manifest) =
            tokio::join!(self.write_trampoline(), self.write_configuration());
        trampoline?;
        manifest?;
        Ok(())
    }

    /// Returns the decompressed trampoline binary
    pub fn decompressed_trampoline() -> &'static [u8] {
        // A static variable to hold the cached decompressed trampoline binary
        static DECOMPRESSED_TRAMPOLINE_BIN: LazyLock<Vec<u8>> = LazyLock::new(|| {
            zstd::decode_all(TRAMPOLINE_BIN)
                .expect("we should be able to decompress trampoline binary")
        });

        &DECOMPRESSED_TRAMPOLINE_BIN
    }

    async fn write_trampoline(&self) -> miette::Result<()> {
        tokio_fs::create_dir_all(self.root_path.join(TRAMPOLINE_CONFIGURATION))
            .await
            .into_diagnostic()?;

        let trampoline_path = self.trampoline_path();

        // We need to check that there's indeed a trampoline at the path
        if !trampoline_path.is_file() {
            tokio_fs::write(&trampoline_path, Trampoline::decompressed_trampoline())
                .await
                .into_diagnostic()?;
        } else if !Trampoline::is_trampoline(&self.trampoline_path()).await? {
            tokio_fs::remove_file(&trampoline_path)
                .await
                .into_diagnostic()?;
            tokio_fs::write(&trampoline_path, Trampoline::decompressed_trampoline())
                .await
                .into_diagnostic()?;
        }

        // If the path doesn't exist yet, create a hard link to the shared trampoline binary
        // If creating a hard link doesn't succeed, try copying
        // Hard-linking might for example fail because the file-system enforces a maximum number of hard-links per file
        if !self.path().exists()
            && tokio_fs::hard_link(&trampoline_path, self.path())
                .await
                .is_err()
        {
            tokio_fs::copy(&trampoline_path, self.path())
                .await
                .into_diagnostic()?;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio_fs::set_permissions(self.path(), std::fs::Permissions::from_mode(0o755))
                .await
                .into_diagnostic()?;
        }

        Ok(())
    }

    /// Writes the configuration file of the trampoline
    async fn write_configuration(&self) -> miette::Result<()> {
        let configuration_string =
            serde_json::to_string_pretty(&self.configuration).into_diagnostic()?;
        tokio_fs::create_dir_all(
            self.configuration()
                .parent()
                .expect("should have a parent folder"),
        )
        .await
        .into_diagnostic()?;

        tokio_fs::write(self.configuration(), configuration_string)
            .await
            .into_diagnostic()?;

        Ok(())
    }

    /// Checks if executable is a saved trampoline
    /// by comparing the file size and then by reading the first 1048 bytes of the file
    pub async fn is_trampoline(path: &Path) -> miette::Result<bool> {
        let mut bin_file = tokio_fs::File::open(path).await.into_diagnostic()?;
        let metadata = bin_file.metadata().await.into_diagnostic()?;
        let file_size = metadata.len();

        if file_size != Trampoline::decompressed_trampoline().len() as u64 {
            return Ok(false);
        }

        let mut buf = vec![0; file_size as usize];
        match bin_file.read_exact(&mut buf).await {
            Ok(_) => Ok(buf == Trampoline::decompressed_trampoline()),
            Err(err) => {
                if err.kind() == ErrorKind::UnexpectedEof {
                    Ok(false)
                } else {
                    Err(err).into_diagnostic()
                }
            }
        }
    }
}

mod tests {
    // Test is_trampoline when it is a trampoline
    #[tokio::test]
    async fn test_is_trampoline() {
        use super::*;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let trampoline_path = dir.path().join("trampoline");
        tokio_fs::write(&trampoline_path, Trampoline::decompressed_trampoline())
            .await
            .unwrap();

        assert!(Trampoline::is_trampoline(&trampoline_path).await.unwrap());
    }

    // Test is_trampoline on simple empty file
    // We want to be sure that eof is properly handled
    #[tokio::test]
    async fn test_is_trampoline_handle_eof() {
        use super::*;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let trampoline_path = dir.path().join("trampoline");
        tokio_fs::write(&trampoline_path, "").await.unwrap();

        assert!(!Trampoline::is_trampoline(&trampoline_path).await.unwrap(),);
    }

    // Test is_trampoline on non existing file
    // it should raise an error
    #[tokio::test]
    async fn test_is_trampoline_err() {
        use super::*;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let trampoline_path = dir.path().join("trampoline");

        assert!(Trampoline::is_trampoline(&trampoline_path).await.is_err());
    }

    // Test is_trampoline on non existing file
    // it should raise an error
    #[tokio::test]
    async fn test_trampoline_is_hardlinked() {
        use super::*;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let trampoline_path = dir.path().join("hardlink_to_trampoline");

        let trampoline = Trampoline::new(
            ExposedName::from_str("test_hardlink").unwrap(),
            dir.path().to_path_buf(),
            Configuration::new(trampoline_path.clone(), String::new(), HashMap::new()),
        );

        trampoline.save().await.unwrap();

        // Check if the original trampoline is saved correctly
        assert!(trampoline.trampoline_path().exists());
        assert!(is_executable::is_executable(trampoline.trampoline_path()));

        // check if we created the hardlink
        // inspired from this:
        // https://github.com/rust-lang/rust/blob/27e38f8fc7efc57b75e9a763d7a0ee44822cd5f7/library/std/src/fs/tests.rs#L949
        assert!(trampoline.path().exists());
        // Fetch metadata for both files
        let shared_metadata = tokio_fs::metadata(trampoline.trampoline_path())
            .await
            .unwrap();
        let linked_metadata = tokio_fs::metadata(trampoline.path()).await.unwrap();
        // Check if the metadata is the same
        assert_eq!(shared_metadata.len(), linked_metadata.len());
    }
}
