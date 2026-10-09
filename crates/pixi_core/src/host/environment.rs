//! Isolated detector environments use Pixi's ordinary solve and install flow.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use pixi_command_dispatcher::{
    BuildEnvironment, CacheDirs, CommandDispatcher, GatewayReporter, InstallPixiEnvironmentSpec,
    Limits, PackagesDir, SolveCondaEnvironmentSpec, offline::exclusions_for_solve,
    reporter::WrappingGatewayReporter,
};
use pixi_config::{CacheKind, Config};
use pixi_path::AbsPathBuf;
use pixi_record::UnresolvedPixiRecord;
use pixi_reporters::TopLevelProgress;
use pixi_spec::DetailedSpec;
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{GenericVirtualPackage, NamedChannelOrUrl, Subdir, prefix::Prefix};
use rattler_environment_digest::environment_digest;
use rattler_networking::LazyClient;
use rattler_repodata_gateway::{AcceptedDetectorRegistration, ChannelRelationsMode, Gateway};
use rattler_virtual_package_detectors::{
    DetectorEnvironment, DetectorEnvironmentProvider, EnvironmentError, ResolvedDetector,
    environment::{detector_spec, prefix_for},
};

use super::HostDetectError;

pub(super) struct PixiEnvironmentProvider<'a> {
    dispatcher: CommandDispatcher,
    root: &'a Path,
    offline: bool,
    progress: Arc<TopLevelProgress>,
}

impl<'a> PixiEnvironmentProvider<'a> {
    pub(super) fn new(
        config: &Config,
        gateway: Gateway,
        client: LazyClient,
        root: &'a Path,
        platform: Subdir,
        virtual_packages: Vec<GenericVirtualPackage>,
    ) -> Result<Self, HostDetectError> {
        let root_dir = AbsPathBuf::new(root.to_owned())
            .map_err(|error| HostDetectError::CacheDir(error.to_string()))?
            .into_assume_dir();
        let packages_dir = AbsPathBuf::new(
            config
                .cache_dir_for(CacheKind::CondaPackages)
                .map_err(|error| HostDetectError::CacheDir(error.to_string()))?,
        )
        .map_err(|error| HostDetectError::CacheDir(error.to_string()))?
        .into_assume_dir();
        // No workspace registry, manifest, lockfile, or detected host is involved.
        let builder = CommandDispatcher::builder()
            .with_gateway(gateway)
            .with_download_client(client)
            .with_cache_dirs(
                CacheDirs::new(root_dir.clone()).with_override::<PackagesDir>(packages_dir),
            )
            .with_root_dir(root_dir)
            .with_channel_config(config.global_channel_config().clone())
            .with_tool_platform(platform, virtual_packages)
            .with_max_download_concurrency(config.max_concurrent_downloads())
            .with_limits(Limits {
                max_concurrent_solves: config.max_concurrent_solves().into(),
                ..Limits::default()
            })
            .with_allow_symbolic_links(config.allow_symbolic_links)
            .with_allow_hard_links(config.allow_hard_links)
            .with_allow_ref_links(config.allow_ref_links)
            .with_offline(config.offline())
            .execute_link_scripts(false);
        crate::rayon_primer::RayonPrimer::prime();
        let progress = TopLevelProgress::from_global();
        let dispatcher = progress.clone().register_with(builder).finish();
        Ok(Self {
            dispatcher,
            root,
            offline: config.offline(),
            progress,
        })
    }

    pub(super) fn progress(&self) -> &Arc<TopLevelProgress> {
        &self.progress
    }
}

#[async_trait]
impl DetectorEnvironmentProvider for PixiEnvironmentProvider<'_> {
    async fn resolve(
        &self,
        registration: &AcceptedDetectorRegistration,
    ) -> Result<ResolvedDetector, EnvironmentError> {
        let (platform, virtual_packages) = self.dispatcher.tool_platform();
        let reporter = self
            .progress
            .create_gateway_reporter(self.progress.registry().allocate());
        let mut query = self
            .dispatcher
            .gateway()
            .query(
                registration.resolution_channels.iter().cloned(),
                [platform, Subdir::NoArch],
                [detector_spec(registration)],
            )
            .recursive(true)
            // Registration discovery has already expanded these channels.
            .channel_relations(ChannelRelationsMode::Disabled)
            .channel_notices(reporter.is_some());
        if let Some(reporter) = reporter {
            query = query.with_reporter(WrappingGatewayReporter(reporter));
        }
        let repodata = query.await?.repodata;
        let excluded_candidates = exclusions_for_solve(
            self.offline,
            self.dispatcher.package_cache(),
            repodata.iter().flat_map(|subdir| subdir.iter()),
        )
        .await
        .map_err(|error| EnvironmentError::Provider(Box::new(error)))?;
        let mut binary_specs = DependencyMap::default();
        binary_specs.insert(
            registration.registration.detector.clone(),
            DetailedSpec {
                channel: Some(NamedChannelOrUrl::Url(
                    registration.channel.base_url.clone().into(),
                )),
                ..DetailedSpec::default()
            }
            .into(),
        );
        let records = self
            .dispatcher
            .solve_conda_environment(SolveCondaEnvironmentSpec {
                name: Some(format!(
                    "detector {}",
                    registration.registration.detector.as_source()
                )),
                binary_specs,
                binary_repodata: repodata,
                platform,
                channels: registration
                    .resolution_channels
                    .iter()
                    .map(|channel| channel.base_url.clone())
                    .collect(),
                // Only built-in capabilities are available, preventing detector recursion.
                virtual_packages: virtual_packages.to_vec(),
                excluded_candidates,
                ..SolveCondaEnvironmentSpec::default()
            })
            .await
            .map_err(|error| EnvironmentError::Provider(Box::new(error)))?
            .into_iter()
            .map(|record| Arc::unwrap_or_clone(record.into_binary().expect("binary-only solve")))
            .collect::<Vec<_>>();
        let digest = environment_digest(&records);
        Ok(ResolvedDetector { records, digest })
    }

    async fn install(
        &self,
        resolved: ResolvedDetector,
    ) -> Result<DetectorEnvironment, EnvironmentError> {
        let prefix_path = prefix_for(self.root, &resolved.digest);
        let prefix =
            Prefix::create(prefix_path.clone()).map_err(|source| EnvironmentError::Guard {
                prefix: prefix_path.clone(),
                source,
            })?;
        let (platform, _) = self.dispatcher.tool_platform();
        let result = self
            .dispatcher
            .install_pixi_environment(InstallPixiEnvironmentSpec {
                name: "virtual package detector".to_owned(),
                records: resolved
                    .records
                    .into_iter()
                    .map(|record| UnresolvedPixiRecord::Binary(Arc::new(record)))
                    .collect(),
                prefix,
                // Binary-only installation needs no build capabilities or source channels.
                build_environment: BuildEnvironment::simple(platform, Vec::new()),
                installed: None,
                ignore_packages: None,
                force_reinstall: Default::default(),
                exclude_newer: None,
                channels: Vec::new(),
                variant_configuration: None,
                variant_files: None,
                inline_packages: Default::default(),
            })
            .await
            .map_err(|error| EnvironmentError::Provider(Box::new(error)))?;
        // The normal installer owns the cross-process fingerprint lock and
        // forces a complete relink after an interrupted installation.
        Ok(DetectorEnvironment {
            prefix: prefix_path,
            installed: !result.transaction.operations.is_empty(),
        })
    }
}
