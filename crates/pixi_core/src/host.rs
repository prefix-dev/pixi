//! What this machine provides, cached for each effective environment solve scope.
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
//! network access and consent, so a [`HostDetector`] prepares detections
//! asynchronously while the workspace is located. Consumers read the cached
//! [`HostDetection`] for their solve channels instead of probing the machine.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io::IsTerminal,
    path::Path,
    sync::{Arc, Mutex},
};

use crate::workspace::grouped_environment::GroupedEnvironment;
use async_trait::async_trait;
use miette::Diagnostic;
use pixi_config::{CacheKind, Config};
use pixi_manifest::platform::{
    PixiPlatform, PixiPlatformError, candidate_subdirs,
    host::{detect_host, host_subdir, platform_from_detected},
    solver_generic_virtual_packages,
};
use pixi_manifest::{
    Feature, FeaturesExt, HasFeaturesIter, HasWorkspaceManifest, WorkspaceManifest,
};
use pixi_utils::reqwest::build_lazy_reqwest_clients;
use rattler_conda_types::{
    Channel, ChannelUrl, GenericVirtualPackage, MatchSpec, PackageName, Subdir,
    virtual_package_detector::DetectorRegistration,
};
use rattler_networking::LazyClient;
use rattler_networking::s3_middleware::S3Config;
use rattler_repodata_gateway::{AcceptedDetectorRegistration, Gateway};
use rattler_shell::environment::EnvironmentSnapshot;
use rattler_virtual_package_detectors::{
    CacheClock, ConfiguredConsent, DetectOptions, OverrideError, detect, limits, merge_results,
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

/// The machine's capabilities are unknown because host detection failed.
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
        Self::detect_for(host_subdir()).await
    }

    /// Runs built-in detection for `subdir` off the async executor.
    pub async fn detect_for(subdir: Subdir) -> Self {
        match tokio::task::spawn_blocking(move || Self::builtin_for(subdir)).await {
            Ok(detection) => detection,
            Err(error) => Self::from_result(
                subdir,
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
    /// Detectors only run for the native platform. With
    /// `PIXI_OVERRIDE_PLATFORM` set to another subdir, their names are absent
    /// unless overridden.
    /// A failure to reach the channels degrades to built-in detection with a
    /// warning. A detector failure discards that detector's names and is reported.
    pub async fn detect(
        &self,
        channels: &[ChannelUrl],
        wanted: WantedNames,
    ) -> Result<HostDetection, HostDetectError> {
        self.detect_for_target(channels, host_subdir(), wanted)
            .await
    }

    /// Detects the virtual packages available to a solve for `target`.
    ///
    /// Registrations come from the target subdir and `noarch`. Detectors
    /// execute only when the target is this machine's native subdir. For
    /// other targets their names are absent unless overridden.
    pub async fn detect_for_target(
        &self,
        channels: &[ChannelUrl],
        target: Subdir,
        wanted: WantedNames,
    ) -> Result<HostDetection, HostDetectError> {
        let builtin = HostDetection::detect_for(target).await;
        if builtin.platform().is_err() {
            return Ok(builtin);
        }
        if matches!(&wanted, WantedNames::Only(names) if names.is_empty()) {
            return Ok(builtin);
        }
        if channels.is_empty() {
            return Ok(builtin);
        }
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
        self.detect_registrations(builtin, &output.registrations, wanted)
            .await
    }

    async fn detect_registrations(
        &self,
        builtin: HostDetection,
        registrations: &[AcceptedDetectorRegistration],
        wanted: WantedNames,
    ) -> Result<HostDetection, HostDetectError> {
        let Ok(builtin_platform) = builtin.platform() else {
            return Ok(builtin);
        };
        if registrations.is_empty() {
            return Ok(builtin);
        }
        let Some(native) = Subdir::current() else {
            return Ok(builtin);
        };
        let subdir = builtin.subdir();

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
            solver_generic_virtual_packages(builtin_platform),
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
            registrations,
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
                     platform {} is not this machine's. Set {} to supply its virtual packages.",
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
            let merged = merge_results(solver_generic_virtual_packages(builtin_platform), &results);
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

    /// Detects virtual packages for a solve of `specs` on `platform`: runs
    /// only the detectors whose names candidate repodata or input specs reference.
    pub async fn detect_for_specs(
        &self,
        channels: &[ChannelUrl],
        platform: Subdir,
        specs: &[MatchSpec],
    ) -> Result<HostDetection, HostDetectError> {
        self.detect_for_specs_with_wanted(
            channels,
            platform,
            specs,
            WantedNames::Only(Default::default()),
        )
        .await
    }

    async fn detect_for_specs_with_wanted(
        &self,
        channels: &[ChannelUrl],
        platform: Subdir,
        specs: &[MatchSpec],
        wanted: WantedNames,
    ) -> Result<HostDetection, HostDetectError> {
        if specs.is_empty() {
            return self.detect_for_target(channels, platform, wanted).await;
        }
        if channels.is_empty() {
            return Ok(HostDetection::detect_for(platform).await);
        }
        let repodata = match self
            .gateway
            .query(
                channels.iter().cloned().map(Channel::from_url),
                [platform, Subdir::NoArch],
                specs.iter().cloned(),
            )
            .recursive(true)
            .virtual_package_detectors(platform)
            .await
        {
            Ok(repodata) => repodata,
            Err(error) => {
                let message = format!(
                    "Could not read repodata to find the virtual packages the solve can \
                     reference, considering all registered detectors: {error}"
                );
                // A missing repodata cache is expected in offline mode.
                if self.config.offline() {
                    tracing::debug!("{message}");
                } else {
                    tracing::warn!("{message}");
                }
                return self
                    .detect_for_target(channels, platform, WantedNames::All)
                    .await;
            }
        };
        for warning in &repodata.warnings {
            tracing::warn!("{warning}");
        }
        let discovery = repodata
            .virtual_package_detectors
            .expect("detector discovery was requested");
        let wanted = match wanted {
            WantedNames::All => WantedNames::All,
            WantedNames::Only(mut names) => {
                names.extend(discovery.wanted_names);
                WantedNames::Only(names)
            }
        };
        self.detect_registrations(
            HostDetection::detect_for(platform).await,
            &discovery.registrations,
            wanted,
        )
        .await
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
        .as_ref()
        .into_iter()
        .flatten()
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

/// The ordered channels and demand that determine an environment's host.
#[derive(PartialEq)]
pub(crate) struct HostScope {
    channels: Vec<ChannelUrl>,
    specs: Vec<MatchSpec>,
    names: BTreeSet<PackageName>,
}

/// The script's effective default environment, before it becomes a workspace.
struct ScriptEnvironment<'a> {
    manifest: &'a WorkspaceManifest,
}

impl<'a> HasWorkspaceManifest<'a> for ScriptEnvironment<'a> {
    fn workspace_manifest(&self) -> &'a WorkspaceManifest {
        self.manifest
    }
}

impl<'a> HasFeaturesIter<'a> for ScriptEnvironment<'a> {
    fn features(&self) -> impl DoubleEndedIterator<Item = &'a Feature> + 'a {
        let manifest = self.manifest;
        let environment = manifest.default_environment();
        environment
            .features
            .iter()
            .map(|name| {
                manifest
                    .feature(name)
                    .expect("feature usage should have been validated upfront")
            })
            .chain((!environment.no_default_feature).then(|| manifest.default_feature()))
    }
}

impl HostScope {
    pub(crate) fn for_environment(
        environment: &GroupedEnvironment<'_>,
    ) -> Result<Self, HostProbeError> {
        let channel_config = environment.channel_config();
        let subdirs = candidate_subdirs(host_subdir());
        let platform_names: HashSet<_> = environment
            .environments()
            .flat_map(|environment| environment.platforms())
            .collect();
        let platforms = environment
            .workspace_manifest()
            .workspace
            .platforms
            .iter()
            .filter(|platform| {
                platform_names.contains(platform.name()) && subdirs.contains(&platform.subdir())
            });
        Self::for_platforms(environment, platforms, &channel_config)
    }

    fn for_script(
        manifest: &WorkspaceManifest,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, HostProbeError> {
        let environment = ScriptEnvironment { manifest };
        if manifest.workspace.platforms.is_empty() {
            // Implicit scripts resolve for the host, including its target dependencies.
            let platform = PixiPlatform::from_subdir(host_subdir());
            return Self::for_platforms(&environment, std::iter::once(&platform), channel_config);
        }
        let subdirs = candidate_subdirs(host_subdir());
        let platform_names = environment.platforms();
        let platforms = manifest.workspace.platforms.iter().filter(|platform| {
            platform_names.contains(platform.name()) && subdirs.contains(&platform.subdir())
        });
        Self::for_platforms(&environment, platforms, channel_config)
    }

    fn for_platforms<'a, 'source>(
        environment: &impl FeaturesExt<'source>,
        platforms: impl IntoIterator<Item = &'a PixiPlatform>,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, HostProbeError> {
        let channels = environment
            .channel_urls(channel_config)
            .map_err(HostProbeError::Channel)?;
        let mut specs = Vec::new();
        let mut names = BTreeSet::new();
        for platform in platforms {
            names.extend(
                platform
                    .customised_virtual_packages()
                    .into_iter()
                    .map(|package| package.name),
            );
            for (name, spec) in environment
                .combined_dependencies(Some(platform))
                .iter_specs()
            {
                if let Some(nameless) = spec
                    .clone()
                    .try_into_nameless_match_spec(channel_config)
                    .map_err(HostProbeError::Spec)?
                {
                    let spec = MatchSpec::from_nameless(nameless, name.clone().into());
                    if !specs.contains(&spec) {
                        specs.push(spec);
                    }
                }
            }
        }
        Ok(Self {
            channels,
            specs,
            names,
        })
    }

    pub(crate) async fn detect(
        &self,
        detector: &HostDetector,
    ) -> Result<HostDetection, HostProbeError> {
        Ok(detector
            .detect_for_specs_with_wanted(
                &self.channels,
                host_subdir(),
                &self.specs,
                WantedNames::Only(self.names.clone()),
            )
            .await?)
    }
}

/// Probes the script's effective default environment for dependency demand and
/// declared virtual packages on host-compatible targets. Implicit scripts use
/// the host target. Declared platforms remain solver requirements.
pub async fn probe_script(
    manifest: &pixi_manifest::WorkspaceManifest,
    channel_config: &rattler_conda_types::ChannelConfig,
    config: Config,
    consent: Arc<dyn DetectorConsent>,
) -> Result<HostDetection, HostProbeError> {
    let scope = HostScope::for_script(manifest, channel_config)?;
    let detector = HostDetector::new(config, consent, Some(s3_config_of(manifest)))?;
    scope.detect(&detector).await
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

#[cfg(test)]
mod tests {
    use rattler_conda_types::NamelessMatchSpec;
    use std::path::Path;

    use crate::Workspace;

    use super::*;

    #[test]
    fn environment_scope_ignores_unused_feature_demand_and_platforms() {
        let subdir = host_subdir();
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            &format!(
                r#"[workspace]
name = "scoped-demand"
channels = []
platforms = [
    {{ name = "plain", platform = "{subdir}" }},
    {{ name = "custom", platform = "{subdir}", test_good = "1" }},
]

[dependencies]
default-consumer = "*"

[feature.plain]
platforms = ["plain"]

[feature.unused]
platforms = ["custom"]

[feature.unused.dependencies]
needs-good = "*"

[environments]
default = {{ features = ["plain"], no-default-feature = true }}
"#
            ),
        )
        .unwrap();
        let scope = HostScope::for_environment(&workspace.default_environment().into()).unwrap();
        assert!(scope.specs.is_empty());
        assert!(scope.names.is_empty());
        let script_scope = HostScope::for_script(
            (&workspace).workspace_manifest(),
            &workspace.channel_config(),
        )
        .unwrap();
        assert!(script_scope.specs.is_empty());
        assert!(script_scope.names.is_empty());
    }

    #[test]
    fn conda_script_scope_includes_extra_feature_and_applicable_targets() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let subdir = host_subdir();
        let foreign = if subdir == Subdir::Linux64 {
            Subdir::LinuxAarch64
        } else {
            Subdir::Linux64
        };
        let channel_config = rattler_conda_types::ChannelConfig {
            root_dir: root.to_owned(),
            ..Config::default().global_channel_config().clone()
        };
        for explicit_platform in [false, true] {
            let (platforms, target) = if explicit_platform {
                (
                    format!(
                        "# [tool.pixi.workspace]\n# platforms = [{{ name = \"capable\", platform = \"{subdir}\", test_good = \"1\" }}, {{ name = \"foreign\", platform = \"{foreign}\", test_foreign = \"1\" }}]\n"
                    ),
                    "capable".to_owned(),
                )
            } else {
                (String::new(), subdir.to_string())
            };
            let source = format!(
                r#"# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "target-consumer"
# [dependencies]
# shared-consumer = ">=1"
{platforms}# [tool.pixi.dependencies]
# shared-consumer = "<4"
# extra-feature-consumer = "*"
# [tool.pixi.target.{target}.dependencies]
# target-consumer = "*"
# [tool.pixi.target.{foreign}.dependencies]
# foreign-consumer = "*"
# /// end-conda-script
"#
            );
            let script = pixi_manifest::script::conda::CondaScriptManifest::from_source(
                root.join("scope.code"),
                source.as_bytes(),
            )
            .unwrap()
            .unwrap();
            let (manifest, _) = script.into_workspace_manifest(None, root).unwrap();
            let scope = HostScope::for_script(&manifest, &channel_config).unwrap();
            assert_eq!(scope.specs.len(), 4);
            for (name, version) in [
                ("shared-consumer", ">=1"),
                ("shared-consumer", "<4"),
                ("extra-feature-consumer", "*"),
                ("target-consumer", "*"),
            ] {
                let expected = MatchSpec::from_nameless(
                    NamelessMatchSpec {
                        version: Some(version.parse().unwrap()),
                        ..Default::default()
                    },
                    PackageName::new_unchecked(name).into(),
                );
                assert!(
                    scope.specs.contains(&expected),
                    "missing {expected} for explicit_platform={explicit_platform}",
                );
            }
            let expected_names = if explicit_platform {
                BTreeSet::from([PackageName::new_unchecked("__test_good")])
            } else {
                BTreeSet::new()
            };
            assert_eq!(scope.names, expected_names);
        }
    }
}
