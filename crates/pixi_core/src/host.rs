//! What this machine provides, detected once and handed around as a value.
//!
//! Selecting a platform, checking whether an environment can run here and
//! solving for the host all need the same facts: the host subdir and the
//! virtual packages the machine offers. Detecting them costs time (CUDA
//! detection loads a driver) and, once channel-registered detectors take part,
//! network access and consent. So detection happens once, asynchronously,
//! while the workspace is located, and every consumer reads the resulting
//! [`HostDetection`] instead of probing the machine itself.

use std::borrow::Cow;

use rattler_conda_types::{GenericVirtualPackage, Subdir};

use pixi_manifest::platform::{
    PixiPlatform,
    host::{detect_host, host_subdir},
};

/// The machine's capabilities are unknown because host detection failed.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("could not detect the virtual packages of this machine: {message}")]
pub struct HostUndetected {
    message: String,
}

/// The host subdir and what the machine provides for it.
#[derive(Clone, Debug)]
pub struct HostDetection {
    subdir: Subdir,
    platform: Result<PixiPlatform, HostUndetected>,
}

impl HostDetection {
    /// Detects the host with pixi's built-in detection, honoring
    /// `PIXI_OVERRIDE_PLATFORM` and `CONDA_OVERRIDE_*`.
    pub fn builtin() -> Self {
        Self::builtin_for(host_subdir())
    }

    /// Detects what this machine provides for `subdir`. For a subdir the
    /// machine cannot run, that is the subdir's baseline.
    pub fn builtin_for(subdir: Subdir) -> Self {
        let platform = detect_host(subdir).map_err(|error| {
            tracing::warn!("Could not detect the virtual packages of this machine: {error}");
            HostUndetected {
                message: error.to_string(),
            }
        });
        Self { subdir, platform }
    }

    /// Runs [`Self::builtin`] off the async executor.
    pub async fn detect() -> Self {
        match tokio::task::spawn_blocking(Self::builtin).await {
            Ok(detection) => detection,
            Err(error) => Self {
                subdir: host_subdir(),
                platform: Err(HostUndetected {
                    message: format!("detection was interrupted: {error}"),
                }),
            },
        }
    }

    /// A detection that reports exactly `platform`.
    pub fn from_platform(platform: PixiPlatform) -> Self {
        Self {
            subdir: platform.subdir(),
            platform: Ok(platform),
        }
    }

    /// The subdir this machine targets.
    pub fn subdir(&self) -> Subdir {
        self.subdir
    }

    /// The host as a platform: the subdir plus the detected virtual packages.
    /// Callers whose result depends on getting this right, anything that
    /// solves or installs, propagate the error.
    pub fn platform(&self) -> Result<&PixiPlatform, HostUndetected> {
        self.platform.as_ref().map_err(Clone::clone)
    }

    /// The virtual packages this machine provides, for "does the host satisfy
    /// this declared platform?" questions. Empty when detection failed, which
    /// fails closed: no declared platform's requirements are met, rather than
    /// pixi assuming its defaults for a machine it could not read.
    /// Detected solver records are borrowed; an injected manifest platform's
    /// declarations are normalized into solver records.
    pub fn capabilities(&self) -> Cow<'_, [GenericVirtualPackage]> {
        match &self.platform {
            Ok(platform) => platform.solver_records(),
            Err(_) => Cow::Borrowed(&[]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_manifest::platform::host::platform_from_detected;
    use rattler_conda_types::{PackageName, Version};

    #[test]
    fn detected_capabilities_preserve_empty_builds() {
        let records = vec![GenericVirtualPackage {
            name: PackageName::try_from("__vendor").unwrap(),
            version: Version::major(7),
            build_string: String::new(),
        }];
        let platform = platform_from_detected(Subdir::Linux64, records.clone()).unwrap();
        let host = HostDetection::from_platform(platform);

        let capabilities = host.capabilities();
        assert_eq!(capabilities.as_ref(), records.as_slice());
    }

    #[test]
    fn injected_manifest_capabilities_use_solver_records() {
        let platform = PixiPlatform::new(
            "injected".try_into().unwrap(),
            Subdir::Linux64,
            vec![
                GenericVirtualPackage {
                    name: "__archspec".parse().unwrap(),
                    version: Version::major(0),
                    build_string: "zen2".to_string(),
                },
                GenericVirtualPackage {
                    name: "__cuda".parse().unwrap(),
                    version: Version::major(13),
                    build_string: String::new(),
                },
            ],
        )
        .unwrap();
        let host = HostDetection::from_platform(platform);

        assert_eq!(
            host.capabilities().as_ref(),
            &[
                GenericVirtualPackage {
                    name: "__archspec".parse().unwrap(),
                    version: Version::major(1),
                    build_string: "zen2".to_string(),
                },
                GenericVirtualPackage {
                    name: "__cuda".parse().unwrap(),
                    version: Version::major(13),
                    build_string: "0".to_string(),
                },
            ]
        );
    }
}
