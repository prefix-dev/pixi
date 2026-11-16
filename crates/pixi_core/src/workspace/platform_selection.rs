//! Command-boundary platform selection. Consumers retain the result through activation.

use miette::Diagnostic;
use pixi_manifest::platform::{
    candidate_subdirs,
    host::{host_baseline, platform_override_is_foreign},
};
use pixi_manifest::{
    EnvironmentName, FeaturesExt, HasWorkspaceManifest, PixiPlatform, PixiPlatformName,
};
use rattler_lock::LockFile;
use serde::Serialize;
use thiserror::Error;

use super::{
    Environment, PlatformSelection,
    errors::{SkippedPlatform, UnsupportedPlatformError},
    virtual_packages::{environment_has_dependencies, environment_has_dependencies_for_platform},
};
use crate::{
    Workspace,
    environment::PlatformNotInEnvironmentError,
    host::HostDetection,
    lock_file::{
        LoadLockFileError, LockFileVersionMismatchError, resolve_lock_platform_for,
        virtual_packages::{
            MachineValidationError, UnknownNames, validate_system_meets_environment_requirements,
        },
    },
};

#[derive(Debug)]
pub enum ResolvedPlatform<'p> {
    Explicit(&'p PixiPlatform),
    Installed(&'p PixiPlatform),
    HostMatch(&'p PixiPlatform),
    LockFallback(&'p PixiPlatform),
    HostBaseline(PixiPlatform),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformSelectionSource {
    Explicit,
    Installed,
    HostMatch,
    LockFallback,
    HostBaseline,
}

impl ResolvedPlatform<'_> {
    pub fn platform(&self) -> &PixiPlatform {
        match self {
            Self::Explicit(platform)
            | Self::Installed(platform)
            | Self::HostMatch(platform)
            | Self::LockFallback(platform) => platform,
            Self::HostBaseline(platform) => platform,
        }
    }

    pub fn source(&self) -> PlatformSelectionSource {
        match self {
            Self::Explicit(_) => PlatformSelectionSource::Explicit,
            Self::Installed(_) => PlatformSelectionSource::Installed,
            Self::HostMatch(_) => PlatformSelectionSource::HostMatch,
            Self::LockFallback(_) => PlatformSelectionSource::LockFallback,
            Self::HostBaseline(_) => PlatformSelectionSource::HostBaseline,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrefixPolicy {
    Reuse,
    Replace,
}

#[derive(Debug, Error, Diagnostic)]
pub enum PlatformResolutionError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    PlatformNotInEnvironment(#[from] PlatformNotInEnvironmentError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    UnsupportedPlatform(#[from] Box<UnsupportedPlatformError>),
    #[error(transparent)]
    #[diagnostic(transparent)]
    RequirementsUnmet(#[from] MachineValidationError),
    #[error("the lock file has no entry for platform '{platform}' in environment '{environment}'")]
    #[diagnostic(help("run `pixi lock` to resolve this environment for the selected platform"))]
    MissingLockPlatform {
        platform: PixiPlatformName,
        environment: EnvironmentName,
    },
}

/// Validates explicit environment membership without loading or resolving a lock.
pub fn explicit_platform<'p>(
    environment: &Environment<'p>,
    name: &PixiPlatformName,
) -> Result<&'p PixiPlatform, PlatformNotInEnvironmentError> {
    environment
        .workspace_manifest()
        .workspace
        .platform_by_name(name)
        .filter(|_| environment.platforms().contains(name))
        .ok_or_else(|| PlatformNotInEnvironmentError {
            platform: name.clone(),
            environment: environment.name().clone(),
        })
}

fn validate_lock_platform(
    environment: &Environment<'_>,
    lock_file: &LockFile,
    platform: &PixiPlatform,
) -> Result<(), PlatformResolutionError> {
    if environment_has_dependencies_for_platform(environment, platform)
        && !lock_file
            .environment(environment.name().as_str())
            .is_some_and(|env| {
                resolve_lock_platform_for(lock_file, platform)
                    .is_some_and(|p| env.packages(p).is_some())
            })
    {
        return Err(PlatformResolutionError::MissingLockPlatform {
            platform: platform.name().clone(),
            environment: environment.name().clone(),
        });
    }
    Ok(())
}

pub fn resolve_operational_platform<'p>(
    environment: &Environment<'p>,
    lock_file: &LockFile,
    explicit: Option<&PixiPlatformName>,
    prefix_policy: PrefixPolicy,
) -> Result<ResolvedPlatform<'p>, PlatformResolutionError> {
    if let Some(name) = explicit {
        let platform = explicit_platform(environment, name)?;
        validate_lock_platform(environment, lock_file, platform)?;
        return Ok(ResolvedPlatform::Explicit(platform));
    }
    if prefix_policy == PrefixPolicy::Reuse
        && let Some(platform) = environment
            .installed_resolved_platform()
            .filter(|p| environment.platforms().contains(p.name()))
    {
        validate_lock_platform(environment, lock_file, platform)?;
        if !platform_override_is_foreign() {
            let host = environment.host_for_cached_run();
            if !candidate_subdirs(host.subdir()).contains(&platform.subdir()) {
                return Err(Box::new(environment.unsupported_platform_error()).into());
            }
            validate_system_meets_environment_requirements(
                lock_file,
                platform,
                environment.name(),
                &host,
                UnknownNames::Unchecked,
            )?;
        }
        return Ok(ResolvedPlatform::Installed(platform));
    }
    let selected = match environment.select_platform(&environment.host(), Some(lock_file)) {
        Ok(PlatformSelection::Declared { platform, .. }) => ResolvedPlatform::HostMatch(platform),
        Ok(PlatformSelection::LockedOnly { platform, .. }) => {
            ResolvedPlatform::LockFallback(platform)
        }
        Err(_) if !environment_has_dependencies(environment) => {
            ResolvedPlatform::HostBaseline(host_baseline())
        }
        Err(error) => return Err(error.into()),
    };
    validate_lock_platform(environment, lock_file, selected.platform())?;
    Ok(selected)
}

/// Why diagnostics continue without the lock file.
#[derive(Debug, Error, Diagnostic)]
pub enum DiagnosticLockError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Load(#[from] LoadLockFileError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    UnsupportedVersion(#[from] LockFileVersionMismatchError),
}

impl DiagnosticLockError {
    /// The error with its causes and help as plain text, for a warning.
    pub fn warning(&self) -> String {
        let mut message = self.to_string();
        let mut source = std::error::Error::source(self);
        while let Some(cause) = source {
            message.push_str(&format!(": {cause}"));
            source = cause.source();
        }
        if let Some(help) = self.help() {
            message.push_str(&format!(
                "\n  help: {}",
                help.to_string().replace('\n', "\n        ")
            ));
        }
        message
    }
}

/// Loads the lock file to refine diagnostic platform selection.
pub async fn load_diagnostic_lock(workspace: &Workspace) -> Result<LockFile, DiagnosticLockError> {
    Ok(workspace.load_lock_file().await?.into_lock_file()?)
}

/// Whether a diagnostic selection made without the lock file can change once
/// the locked requirements are checked. Only selections that do not depend on
/// the lock file are final: explicit choices, installed prefixes, and the host
/// baseline of a dependency-free environment.
pub fn diagnostic_selection_needs_lock(selection: Option<&ResolvedPlatform<'_>>) -> bool {
    !matches!(
        selection,
        Some(
            ResolvedPlatform::Explicit(_)
                | ResolvedPlatform::Installed(_)
                | ResolvedPlatform::HostBaseline(_)
        )
    )
}

pub fn resolve_diagnostic_platform<'p>(
    environment: &Environment<'p>,
    existing_lock: Option<&LockFile>,
) -> Option<ResolvedPlatform<'p>> {
    resolve_diagnostic_platform_with_host(environment, &environment.host(), existing_lock).0
}

/// Supplies diagnostic host details without detecting or selecting a second time.
pub fn resolve_diagnostic_platform_with_host<'p>(
    environment: &Environment<'p>,
    host: &HostDetection,
    existing_lock: Option<&LockFile>,
) -> (Option<ResolvedPlatform<'p>>, Vec<SkippedPlatform>) {
    if let Some(platform) = environment
        .installed_resolved_platform()
        .filter(|p| environment.platforms().contains(p.name()))
    {
        return (Some(ResolvedPlatform::Installed(platform)), Vec::new());
    }
    match environment.select_platform(host, existing_lock) {
        Ok(PlatformSelection::Declared { platform, skipped }) => {
            (Some(ResolvedPlatform::HostMatch(platform)), skipped)
        }
        Ok(PlatformSelection::LockedOnly { platform, skipped }) => {
            (Some(ResolvedPlatform::LockFallback(platform)), skipped)
        }
        Err(_) if !environment_has_dependencies(environment) => (
            Some(ResolvedPlatform::HostBaseline(host_baseline())),
            Vec::new(),
        ),
        Err(error) => (None, error.skipped_platforms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Workspace,
        environment::{
            EnvironmentFile, LockedEnvironmentHash, PlatformData, write_environment_file,
        },
    };
    use rattler_conda_types::Subdir;

    fn lock(subdir: Subdir, platforms: &[&str]) -> LockFile {
        let rows = platforms
            .iter()
            .map(|name| {
                format!("- name: {name}\n  subdir: {subdir}\n  virtual-packages:\n  - __cuda=99\n")
            })
            .collect::<String>();
        let packages = platforms
            .iter()
            .map(|name| {
                format!(
                    "      {name}:\n      - conda: https://example.com/{subdir}/foo-1.0-h0.conda\n"
                )
            })
            .collect::<String>();
        LockFile::from_str_with_base_directory(
            &format!("version: 7\nplatforms:\n{rows}environments:\n  default:\n    channels: []\n    packages:\n{packages}packages:\n- conda: https://example.com/{subdir}/foo-1.0-h0.conda\n"),
            None,
        ).unwrap()
    }

    #[test]
    fn installed_prefix_wins_only_while_reused() {
        temp_env::with_var("PIXI_OVERRIDE_PLATFORM", None::<&str>, || {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("pixi.toml");
            let mut workspace = Workspace::from_str(
                &path,
                r#"
                [workspace]
                name = "selection"
                channels = []
                platforms = [
                    { name = "first", platform = "linux-64", cuda = "99" },
                    { name = "second", platform = "linux-64", cuda = "99" },
                ]
                [dependencies]
                foo = "*"
            "#,
            )
            .unwrap();
            workspace.host =
                HostDetection::from_platform(PixiPlatform::from_subdir(Subdir::Linux64)).into();
            let environment = workspace.default_environment();
            let second = explicit_platform(&environment, &"second".parse().unwrap()).unwrap();
            write_environment_file(
                &environment.dir(),
                EnvironmentFile {
                    manifest_path: path,
                    environment_name: "default".to_string(),
                    pixi_version: "test".to_string(),
                    environment_lock_file_hash: LockedEnvironmentHash::invalid(),
                    resolved_platform: Some(PlatformData::from(second)),
                    minimum_supported_platform: None,
                    detector_virtual_packages: Default::default(),
                    source_fingerprints: Default::default(),
                },
            )
            .unwrap();
            let lock = lock(Subdir::Linux64, &["first", "second"]);
            let reused =
                resolve_operational_platform(&environment, &lock, None, PrefixPolicy::Reuse)
                    .unwrap();
            assert!(matches!(reused, ResolvedPlatform::Installed(_)));
            assert_eq!(reused.platform().name().as_str(), "second");
            let replaced =
                resolve_operational_platform(&environment, &lock, None, PrefixPolicy::Replace)
                    .unwrap();
            assert!(matches!(replaced, ResolvedPlatform::LockFallback(_)));
            assert_eq!(replaced.platform().name().as_str(), "first");
            let incomplete_lock = self::lock(Subdir::Linux64, &["second"]);
            let selected = resolve_operational_platform(
                &environment,
                &incomplete_lock,
                None,
                PrefixPolicy::Replace,
            )
            .unwrap();
            assert_eq!(selected.platform().name().as_str(), "second");
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                use crate::lock_file::{LockFileDerivedData, UpdateMode};
                let dispatcher = workspace.command_dispatcher_builder(None).unwrap().finish();
                let data = LockFileDerivedData::from_input_lock_file(
                    &workspace,
                    lock,
                    dispatcher.package_cache().clone(),
                    dispatcher,
                    Default::default(),
                );
                let stale = data
                    .resolve_platform(
                        &environment,
                        PrefixPolicy::Replace,
                        UpdateMode::QuickValidate,
                    )
                    .await
                    .unwrap();
                assert_eq!(stale.platform().name().as_str(), "first");

                let mut marker = crate::environment::read_environment_file(&environment.dir())
                    .unwrap()
                    .unwrap();
                marker.environment_lock_file_hash = LockedEnvironmentHash::from_environment(
                    data.as_lock_file().environment("default").unwrap(),
                    Some(second),
                );
                write_environment_file(&environment.dir(), marker).unwrap();
                let cached = data
                    .resolve_platform(
                        &environment,
                        PrefixPolicy::Replace,
                        UpdateMode::QuickValidate,
                    )
                    .await
                    .unwrap();
                assert!(matches!(cached, ResolvedPlatform::Installed(_)));
                assert_eq!(cached.platform().name().as_str(), "second");
                let rebuilding = data
                    .resolve_platform(&environment, PrefixPolicy::Replace, UpdateMode::Revalidate)
                    .await
                    .unwrap();
                assert_eq!(rebuilding.platform().name().as_str(), "first");
            });
        });
    }

    #[test]
    fn host_baseline_does_not_hide_unresolved_dependencies_or_explicit_membership() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pixi.toml");
        let manifest = "[workspace]\nname = 'empty'\nchannels = []\nplatforms = []\n[tasks]\nhello = 'echo hello'\n";
        let workspace = Workspace::from_str(&path, manifest).unwrap();
        let environment = workspace.default_environment();
        let selection = resolve_operational_platform(
            &environment,
            &LockFile::default(),
            None,
            PrefixPolicy::Reuse,
        )
        .unwrap();
        assert!(matches!(selection, ResolvedPlatform::HostBaseline(_)));
        assert!(
            environment
                .tasks(Some(selection.platform()))
                .unwrap()
                .keys()
                .any(|name| name.as_str() == "hello")
        );
        assert!(
            resolve_operational_platform(
                &environment,
                &LockFile::default(),
                Some(&"linux-64".parse().unwrap()),
                PrefixPolicy::Reuse
            )
            .is_err()
        );

        let workspace =
            Workspace::from_str(&path, &format!("{manifest}\n[dependencies]\nfoo = '*'\n"))
                .unwrap();
        let environment = workspace.default_environment();
        assert!(
            resolve_operational_platform(
                &environment,
                &LockFile::default(),
                None,
                PrefixPolicy::Reuse
            )
            .is_err()
        );
        assert!(resolve_diagnostic_platform(&environment, None).is_none());
    }

    #[test]
    fn emulated_lock_fallback_warns_only_when_activated() {
        temp_env::with_var("PIXI_OVERRIDE_PLATFORM", None::<&str>, || {
            let directory = tempfile::tempdir().unwrap();
            let mut workspace = Workspace::from_str(
                &directory.path().join("pixi.toml"),
                r#"
                [workspace]
                name = "emulation"
                channels = []
                platforms = [{ name = "intel", platform = "osx-64", cuda = "99" }]
                [dependencies]
                foo = "*"
            "#,
            )
            .unwrap();
            workspace.host =
                HostDetection::from_platform(PixiPlatform::from_subdir(Subdir::OsxArm64)).into();
            let environment = workspace.default_environment();
            let lock = lock(Subdir::Osx64, &["intel"]);
            let warning = workspace
                .pixi_dir()
                .join(pixi_consts::consts::ONE_TIME_MESSAGES_DIR)
                .join("macos-emulation-warn");
            let accidental = workspace
                .pixi_dir()
                .join(pixi_consts::consts::ONE_TIME_MESSAGES_DIR)
                .join("runs-by-accident-default");
            assert!(resolve_diagnostic_platform(&environment, None).is_none());
            let diagnostic = resolve_diagnostic_platform(&environment, Some(&lock)).unwrap();
            assert!(matches!(diagnostic, ResolvedPlatform::LockFallback(_)));
            assert!(!warning.exists());
            assert!(!accidental.exists());
            for policy in [PrefixPolicy::Reuse, PrefixPolicy::Replace] {
                let selected =
                    resolve_operational_platform(&environment, &lock, None, policy).unwrap();
                assert!(matches!(selected, ResolvedPlatform::LockFallback(_)));
                super::super::virtual_packages::verify_resolved_platform(&environment, &selected)
                    .unwrap();
                environment.emit_emulation_warning(selected.platform());
                assert!(warning.exists());
                assert!(accidental.exists());
            }
        });
    }
}
