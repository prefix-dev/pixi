//! What this machine provides, detected once and handed around as a value.
//!
//! Selecting a platform, checking whether an environment can run here and
//! solving for the host all need the same facts: the host subdir and the
//! virtual packages the machine offers. Two sources contribute them:
//!
//! - pixi's built-in detection of the CEP 30 virtual packages, and
//! - the virtual package detectors that channels register in their repodata,
//!   which pixi installs and runs once the user consented to them.
//!
//! Detecting costs time (CUDA detection loads a driver), and detectors add
//! network access and consent, so a [`HostDetector`] runs once, asynchronously,
//! while the workspace is located, and every consumer reads the resulting
//! [`HostDetection`] instead of probing the machine itself.

use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashMap, HashSet},
    io::IsTerminal,
    path::Path,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use miette::Diagnostic;
use pixi_config::{CacheKind, Config};
use pixi_manifest::platform::{
    PixiPlatform, PixiPlatformError, candidate_subdirs,
    host::{detect_host, host_subdir, platform_from_detected},
    solver_generic_virtual_packages,
};
use pixi_utils::reqwest::build_lazy_reqwest_clients;
use rattler_conda_types::{
    Channel, ChannelUrl, GenericVirtualPackage, MatchSpec, PackageName, Subdir,
    virtual_package_detector::DetectorRegistration,
};
use rattler_networking::LazyClient;
use rattler_networking::s3_middleware::S3Config;
use rattler_repodata_gateway::Gateway;
use rattler_shell::environment::EnvironmentSnapshot;
use rattler_virtual_package_detectors::{
    CacheClock, ConfiguredConsent, DetectOptions, OverrideError, detect, limits, merge_results,
    referenced_virtual_packages,
};
use thiserror::Error;

pub use rattler_virtual_package_detectors::{
    Consent, ConsentRequest, DenyAll, DetectedValue, DetectionSource, DetectorConsent,
    DetectorResult, SkipReason, SkippedRegistration, WantedNames,
};

mod environment;
mod interactive;

pub use interactive::InteractiveConsent;

/// How many detectors are prepared and run at the same time.
const DETECTOR_CONCURRENCY: usize = 4;

/// Asks about untrusted detector channels in a terminal, otherwise skips them.
pub fn cli_detector_consent(project_root: Option<&Path>) -> Arc<dyn DetectorConsent> {
    if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        Arc::new(InteractiveConsent::new(project_root))
    } else {
        Arc::new(NonInteractiveConsent::default())
    }
}

/// Detection of the host failed; the machine's capabilities are unknown.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("could not detect the virtual packages of this machine: {message}")]
pub struct HostUndetected {
    message: String,
}

/// A detector that failed, as reported to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectorFailureReport {
    /// The registration's origin: the registering channel's base URL.
    pub origin: ChannelUrl,
    /// The detector package.
    pub detector: PackageName,
    /// What went wrong, with its causes.
    pub message: String,
    /// What the detector wrote to standard error, where it ran at all.
    pub stderr: Option<String>,
}

/// The host subdir and what the machine provides for it.
#[derive(Clone, Debug)]
pub struct HostDetection {
    subdir: Subdir,
    platform: Result<PixiPlatform, HostUndetected>,
    results: Vec<DetectorResult>,
    failures: Vec<DetectorFailureReport>,
    skipped: Vec<SkippedRegistration>,
}

impl HostDetection {
    /// Detects the host with pixi's built-in detection, honoring
    /// `PIXI_OVERRIDE_PLATFORM` and `CONDA_OVERRIDE_*`.
    pub fn builtin() -> Self {
        Self::builtin_for(host_subdir())
    }

    /// Detects what this machine provides for `subdir` with the built-in
    /// detection. For a subdir the machine cannot run, that is the subdir's
    /// baseline.
    pub fn builtin_for(subdir: Subdir) -> Self {
        static WARNED: std::sync::Once = std::sync::Once::new();
        let platform = detect_host(subdir).map_err(|error| {
            WARNED.call_once(|| {
                tracing::warn!("Could not detect the virtual packages of this machine: {error}");
            });
            HostUndetected {
                message: error.to_string(),
            }
        });
        Self::from_result(subdir, platform)
    }

    /// Runs [`Self::builtin`] off the async executor.
    pub async fn detect() -> Self {
        match tokio::task::spawn_blocking(Self::builtin).await {
            Ok(detection) => detection,
            Err(error) => Self::from_result(
                host_subdir(),
                Err(HostUndetected {
                    message: format!("detection was interrupted: {error}"),
                }),
            ),
        }
    }

    /// A detection that reports exactly `platform`.
    pub fn from_platform(platform: PixiPlatform) -> Self {
        Self::from_result(platform.subdir(), Ok(platform))
    }

    fn from_result(subdir: Subdir, platform: Result<PixiPlatform, HostUndetected>) -> Self {
        Self {
            subdir,
            platform,
            results: Vec::new(),
            failures: Vec::new(),
            skipped: Vec::new(),
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
    pub fn capabilities(&self) -> &[GenericVirtualPackage] {
        match &self.platform {
            Ok(platform) => platform.declared_virtual_packages(),
            Err(_) => &[],
        }
    }

    /// The results detectors and `CONDA_OVERRIDE_*` variables contributed,
    /// with their sources.
    pub fn detector_results(&self) -> &[DetectorResult] {
        &self.results
    }

    /// The detectors that failed. Their names are absent unless overridden.
    pub fn detector_failures(&self) -> &[DetectorFailureReport] {
        &self.failures
    }

    /// The registered detectors that did not run, and why.
    pub fn skipped_detectors(&self) -> &[SkippedRegistration] {
        &self.skipped
    }
}

/// Why a host probe with detectors could not complete.
#[derive(Debug, Error, Diagnostic)]
pub enum HostDetectError {
    /// A `CONDA_OVERRIDE_*` variable of a detector-provided name is invalid.
    /// The detector protocol requires this to be an error, not a fallback.
    #[error(transparent)]
    Override(#[from] OverrideError),

    /// The cache directories could not be determined.
    #[error("could not determine the cache directory for virtual package detectors: {0}")]
    CacheDir(String),

    /// The download client could not be built.
    #[error("could not build the download client: {0}")]
    Client(String),
}

/// Skips untrusted detector channels and warns once per channel.
#[derive(Debug, Default)]
pub struct NonInteractiveConsent {
    warned: Mutex<HashSet<ChannelUrl>>,
}

impl NonInteractiveConsent {
    fn warn_once(&self, channel: &Channel) {
        let key = channel.base_url.clone();
        let mut warned = self
            .warned
            .lock()
            .expect("the consent warning set is never poisoned");
        if !warned.insert(key) {
            return;
        }
        tracing::warn!(
            "Detectors from {} do not run without your consent. Trust this channel with \
             `pixi config set --shared 'virtual-package-detectors.consent.{}' allow`, \
             or use `--local` for this repository. Set `deny` to block its detectors.",
            channel.base_url,
            toml_edit::Key::new(channel.base_url.as_str().trim_end_matches('/')),
        );
    }
}

#[async_trait]
impl DetectorConsent for NonInteractiveConsent {
    async fn decide(&self, request: &ConsentRequest<'_>) -> Consent {
        self.warn_once(request.channel);
        Consent::Deny
    }

    fn decide_before_resolving(
        &self,
        channel: &Channel,
        _registration: &DetectorRegistration,
    ) -> Option<Consent> {
        self.warn_once(channel);
        Some(Consent::Deny)
    }

    fn can_allow(&self) -> bool {
        false
    }
}

/// Produces [`HostDetection`]s: built-in detection plus the channel-registered
/// detectors the configuration and the consent policy allow.
pub struct HostDetector {
    config: Config,
    consent: Arc<dyn DetectorConsent>,
    client: LazyClient,
    gateway: Gateway,
}

impl HostDetector {
    /// A detector for `config`, deciding on unknown detectors with
    /// `consent`. Decisions stored in the configuration take precedence.
    pub fn new(
        config: Config,
        consent: Arc<dyn DetectorConsent>,
        s3_config: Option<HashMap<String, S3Config>>,
    ) -> Result<Self, HostDetectError> {
        let (_, client) = build_lazy_reqwest_clients(Some(&config), s3_config)
            .map_err(|error| HostDetectError::Client(error.to_string()))?;
        let gateway = config.gateway().with_client(client.clone()).finish();
        Ok(Self::with_gateway(config, consent, client, gateway))
    }

    /// A detector that reuses an existing gateway and download client.
    pub fn with_gateway(
        config: Config,
        consent: Arc<dyn DetectorConsent>,
        client: LazyClient,
        gateway: Gateway,
    ) -> Self {
        Self {
            config,
            consent,
            client,
            gateway,
        }
    }

    /// Detects the host: built-in detection, plus the detectors that
    /// `channels` register and that report one of the `wanted` names.
    ///
    /// Detectors only run for the native platform; with
    /// `PIXI_OVERRIDE_PLATFORM` set, their names are absent unless overridden.
    /// A failure to reach the channels degrades to built-in detection with a
    /// warning; a detector failure discards that detector's names and is
    /// reported.
    pub async fn detect(
        &self,
        channels: &[ChannelUrl],
        wanted: WantedNames,
    ) -> Result<HostDetection, HostDetectError> {
        let builtin = HostDetection::detect().await;
        let Ok(builtin_platform) = builtin.platform().cloned() else {
            return Ok(builtin);
        };
        if matches!(&wanted, WantedNames::Only(names) if names.is_empty()) {
            return Ok(builtin);
        }
        if channels.is_empty() {
            return Ok(builtin);
        }
        let Some(native) = Subdir::current() else {
            return Ok(builtin);
        };
        let subdir = builtin.subdir();

        let output = match self
            .gateway
            .virtual_package_detectors(
                channels.iter().cloned().map(Channel::from_url),
                [subdir, Subdir::NoArch],
            )
            .await
        {
            Ok(output) => output,
            Err(error) => {
                tracing::warn!(
                    "Could not read the virtual package detector registrations of the channels, \
                     using pixi's built-in detection only: {error}"
                );
                return Ok(builtin);
            }
        };
        for warning in &output.warnings {
            tracing::warn!("{warning}");
        }
        if output.registrations.is_empty() {
            return Ok(builtin);
        }

        let root = self
            .config
            .cache_dir_for(CacheKind::VirtualPackageDetectors)
            .map_err(|error| HostDetectError::CacheDir(error.to_string()))?;
        let environment_provider = environment::PixiEnvironmentProvider::new(
            &self.config,
            self.gateway.clone(),
            self.client.clone(),
            &root,
            native,
            solver_generic_virtual_packages(&builtin_platform),
        )?;
        let _clear_progress = pixi_reporters::TopLevelProgress::clear_when_done(Some(
            environment_provider.progress(),
        ));
        let consent = ConfiguredConsent::new(
            self.config.virtual_package_detectors.clone(),
            self.consent.clone(),
        );
        let environment = EnvironmentSnapshot::from_system();
        let outcome = detect(
            &output.registrations,
            DetectOptions {
                environment_provider: &environment_provider,
                environment: &environment,
                root: &root,
                host_platform: native,
                target_platform: subdir,
                timeout: self
                    .config
                    .virtual_package_detectors
                    .timeout()
                    .unwrap_or(limits::DEFAULT_TIMEOUT),
                consent: &consent,
                wanted,
                // A policy that may ask the user gets one detector at a time,
                // so nothing else is being installed while a question is up.
                concurrency: if self.consent.can_allow() {
                    1
                } else {
                    DETECTOR_CONCURRENCY
                },
                clock: CacheClock::current(),
            },
        )
        .await?;

        let mut failures: Vec<DetectorFailureReport> = outcome
            .failures
            .iter()
            .map(|failure| {
                let mut message = failure.error.to_string();
                let mut source = std::error::Error::source(&failure.error);
                while let Some(cause) = source {
                    message.push_str(": ");
                    message.push_str(&cause.to_string());
                    source = cause.source();
                }
                DetectorFailureReport {
                    origin: failure.origin.clone(),
                    detector: failure.detector.clone(),
                    message,
                    stderr: failure.stderr.clone(),
                }
            })
            .collect();
        for failure in &failures {
            match &failure.stderr {
                Some(stderr) if !stderr.trim().is_empty() => tracing::warn!(
                    "The virtual package detector '{}' of {} failed, its virtual packages are \
                     absent: {}\n{}",
                    failure.detector.as_source(),
                    failure.origin,
                    failure.message,
                    stderr.trim_end()
                ),
                _ => tracing::warn!(
                    "The virtual package detector '{}' of {} failed, its virtual packages are \
                     absent: {}",
                    failure.detector.as_source(),
                    failure.origin,
                    failure.message
                ),
            }
        }
        for skipped in &outcome.skipped {
            match &skipped.reason {
                SkipReason::TargetIsNotHost { override_variables } => tracing::warn!(
                    "Not running the virtual package detector '{}' of {} because the target \
                     platform {} is not this machine's; set {} to supply its virtual packages.",
                    skipped.detector.as_source(),
                    skipped.origin,
                    subdir,
                    override_variables.join(", ")
                ),
                SkipReason::NoWantedName | SkipReason::ConsentDenied => tracing::debug!(
                    "Not running the virtual package detector '{}' of {}: {:?}",
                    skipped.detector.as_source(),
                    skipped.origin,
                    skipped.reason
                ),
            }
        }

        let mut results = outcome.results;
        let platform = loop {
            let merged =
                merge_results(solver_generic_virtual_packages(&builtin_platform), &results);
            match platform_from_detected(subdir, merged) {
                Ok(platform) => break Ok(platform),
                Err(error) => {
                    // Validate the complete set first: separate detectors can
                    // supply CUDA and its architecture in either order.
                    let offending = match &error {
                        PixiPlatformError::CudaArchRequiresCuda => {
                            let removed_cuda = builtin_platform
                                .declared_virtual_packages()
                                .iter()
                                .any(|package| package.name.as_normalized() == "__cuda")
                                .then(|| {
                                    results.iter().find(|result| {
                                        result.name.as_normalized() == "__cuda"
                                            && matches!(result.value, DetectedValue::Absent)
                                            && matches!(
                                                result.source,
                                                DetectionSource::Detector { .. }
                                            )
                                    })
                                })
                                .flatten();
                            removed_cuda.or_else(|| {
                                results.iter().find(|result| {
                                    result.name.as_normalized() == "__cuda_arch"
                                        && matches!(result.value, DetectedValue::Present(_))
                                        && matches!(result.source, DetectionSource::Detector { .. })
                                })
                            })
                        }
                        _ => None,
                    };
                    let Some(DetectorResult {
                        source:
                            DetectionSource::Detector {
                                origin, detector, ..
                            },
                        ..
                    }) = offending
                    else {
                        break Err(HostUndetected {
                            message: error.to_string(),
                        });
                    };
                    let origin = origin.clone();
                    let detector = detector.clone();
                    tracing::warn!(
                        "The virtual package detector '{}' of {} produced an invalid host \
                         platform, its results are ignored: {error}",
                        detector.as_source(),
                        origin,
                    );
                    failures.push(DetectorFailureReport {
                        origin: origin.clone(),
                        detector: detector.clone(),
                        message: error.to_string(),
                        stderr: None,
                    });
                    results.retain(|result| !matches!(
                        &result.source,
                        DetectionSource::Detector { origin: result_origin, detector: result_detector, .. }
                            if result_origin == &origin && result_detector == &detector
                    ));
                }
            }
        };
        Ok(HostDetection {
            subdir,
            platform,
            results,
            failures,
            skipped: outcome.skipped,
        })
    }

    /// Detects the host for a solve of `specs`: runs only the detectors whose
    /// names the repodata that `specs` pull in can reference.
    pub async fn detect_for_specs(
        &self,
        channels: &[ChannelUrl],
        platform: Subdir,
        specs: &[MatchSpec],
    ) -> Result<HostDetection, HostDetectError> {
        if channels.is_empty() || specs.is_empty() {
            return Ok(HostDetection::detect().await);
        }
        let repodata = match self
            .gateway
            .query(
                channels.iter().cloned().map(Channel::from_url),
                [platform, Subdir::NoArch],
                specs.iter().cloned(),
            )
            .recursive(true)
            .await
        {
            Ok(repodata) => repodata,
            Err(error) => {
                let message = format!(
                    "Could not read repodata to find the virtual packages the solve can \
                     reference, using pixi's built-in detection only: {error}"
                );
                // Offline was asked for; a missing cache is expected then.
                if self.config.offline() {
                    tracing::debug!("{message}");
                } else {
                    tracing::warn!("{message}");
                }
                return Ok(HostDetection::detect().await);
            }
        };
        let mut wanted: BTreeSet<PackageName> = referenced_virtual_packages(
            repodata
                .iter()
                .flat_map(|subdir| subdir.iter())
                .map(|record| &record.package_record),
        );
        wanted.extend(
            specs
                .iter()
                .filter_map(|spec| spec.name.as_exact())
                .filter(|name| name.as_normalized().starts_with("__"))
                .cloned(),
        );
        self.detect(channels, WantedNames::Only(wanted)).await
    }
}

/// Why the host could not be probed for a manifest.
#[derive(Debug, Error, Diagnostic)]
pub enum HostProbeError {
    /// The detection itself failed.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Detect(#[from] HostDetectError),

    /// A channel of the manifest could not be resolved to a URL.
    #[error("could not resolve a channel of the manifest")]
    Channel(#[source] rattler_conda_types::ParseChannelError),

    /// A dependency of the manifest could not be turned into a match spec.
    #[error("could not turn a dependency of the manifest into a match spec")]
    Spec(#[source] pixi_spec::SpecConversionError),

    /// A PEP 723 script could not be read as a manifest.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Script(#[from] pixi_manifest::script::ScriptManifestError),

    /// A conda-script file could not be read as a manifest.
    #[error(transparent)]
    #[diagnostic(transparent)]
    CondaScript(#[from] Box<pixi_manifest::script::conda::CondaScriptError>),

    /// The download client or the repodata gateway could not be set up.
    #[error("could not set up the download client: {0}")]
    Client(String),
}

/// The S3 client options a manifest declares.
pub fn s3_config_of(manifest: &pixi_manifest::WorkspaceManifest) -> HashMap<String, S3Config> {
    manifest
        .workspace
        .s3_options
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                S3Config::Custom {
                    endpoint_url: value.endpoint_url.clone(),
                    region: value.region.clone(),
                    addressing_style: if value.force_path_style {
                        rattler_networking::s3_middleware::S3AddressingStyle::Path
                    } else {
                        rattler_networking::s3_middleware::S3AddressingStyle::VirtualHost
                    },
                    credentials_provider: None,
                },
            )
        })
        .collect()
}

/// The workspace-wide union of channels, highest priority first. Equal
/// priorities retain workspace-then-feature declaration order.
pub fn manifest_channels(
    manifest: &pixi_manifest::WorkspaceManifest,
    channel_config: &rattler_conda_types::ChannelConfig,
) -> Result<Vec<ChannelUrl>, HostProbeError> {
    let mut urls = Vec::new();
    let mut named: Vec<_> = manifest
        .workspace
        .channels
        .iter()
        .chain(
            manifest
                .features()
                .values()
                .filter_map(|feature| feature.channels.as_ref())
                .flatten(),
        )
        .collect();
    named.sort_by_key(|channel| Reverse(channel.priority.unwrap_or(0)));
    for channel in named {
        let url = channel
            .channel
            .clone()
            .into_base_url(channel_config)
            .map_err(HostProbeError::Channel)?;
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    Ok(urls)
}

/// Probes the host using the manifest's prioritized channel union, and only
/// the detectors whose names one of its platforms declares.
pub async fn probe_workspace(
    manifest: &pixi_manifest::WorkspaceManifest,
    channel_config: &rattler_conda_types::ChannelConfig,
    config: Config,
    consent: Arc<dyn DetectorConsent>,
) -> Result<HostDetection, HostProbeError> {
    // Only platforms this machine could select can want a name here.
    let subdirs = candidate_subdirs(host_subdir());
    let wanted = wanted_names_of_platforms(
        manifest
            .workspace
            .platforms
            .iter()
            .filter(|platform| subdirs.contains(&platform.subdir())),
    );
    if matches!(&wanted, WantedNames::Only(names) if names.is_empty()) {
        return Ok(HostDetection::detect().await);
    }
    let channels = manifest_channels(manifest, channel_config)?;
    let detector = HostDetector::new(config, consent, Some(s3_config_of(manifest)))?;
    Ok(detector.detect(&channels, wanted).await?)
}

/// Probes the host for a script: only the detectors whose names the repodata
/// of the script's dependencies can reference.
pub async fn probe_script(
    manifest: &pixi_manifest::WorkspaceManifest,
    channel_config: &rattler_conda_types::ChannelConfig,
    config: Config,
    consent: Arc<dyn DetectorConsent>,
) -> Result<HostDetection, HostProbeError> {
    let channels = manifest_channels(manifest, channel_config)?;
    let detector = HostDetector::new(config, consent, Some(s3_config_of(manifest)))?;
    let mut specs = Vec::new();
    if let Some(dependencies) = manifest.default_feature().combined_dependencies(None) {
        for (name, spec) in dependencies.iter_specs() {
            if let Some(nameless) = spec
                .clone()
                .try_into_nameless_match_spec(channel_config)
                .map_err(HostProbeError::Spec)?
            {
                specs.push(MatchSpec::from_nameless(nameless, name.clone().into()));
            }
        }
    }
    Ok(detector
        .detect_for_specs(&channels, host_subdir(), &specs)
        .await?)
}

/// Probes the host for a conda-script file located in `root`, before the
/// script is turned into a workspace.
pub async fn probe_conda_script(
    script: &pixi_manifest::script::conda::CondaScriptManifest,
    root: &std::path::Path,
    config: &Config,
    consent: Arc<dyn DetectorConsent>,
) -> Result<HostDetection, HostProbeError> {
    let (manifest, _) = script
        .into_workspace_manifest(None, root)
        .map_err(Box::new)?;
    let channel_config = rattler_conda_types::ChannelConfig {
        root_dir: root.to_owned(),
        ..config.global_channel_config().clone()
    };
    probe_script(&manifest, &channel_config, config.clone(), consent).await
}

/// Probes the host for a PEP 723 script located in `root`, before the script
/// is turned into a workspace. A script that declares no channels is probed
/// with the configured default channels, like the workspace it becomes.
pub async fn probe_pep723_script(
    script: &pixi_manifest::script::ScriptManifest,
    root: &std::path::Path,
    config: &Config,
    consent: Arc<dyn DetectorConsent>,
) -> Result<HostDetection, HostProbeError> {
    let (mut manifest, _) = script.clone().into_workspace_manifest()?;
    if !script.workspace_config()?.channels_explicit {
        manifest.workspace.channels = config
            .default_channels()
            .into_iter()
            .map(pixi_manifest::PrioritizedChannel::from)
            .collect();
    }
    let channel_config = rattler_conda_types::ChannelConfig {
        root_dir: root.to_owned(),
        ..config.global_channel_config().clone()
    };
    probe_script(&manifest, &channel_config, config.clone(), consent).await
}

/// The virtual package names the workspace's declared platforms ask for
/// beyond their subdir's baseline. A detector registered for a built-in name
/// replaces the built-in detection of that name.
pub fn wanted_names_of_platforms<'a>(
    platforms: impl IntoIterator<Item = &'a PixiPlatform>,
) -> WantedNames {
    WantedNames::Only(
        platforms
            .into_iter()
            .flat_map(|platform| platform.customised_virtual_packages())
            .map(|package| package.name)
            .collect(),
    )
}
