use std::collections::{BTreeMap, HashSet};

use futures::StreamExt;
use itertools::Either;
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_spec::{BinarySpec, PixiSpec, SourceAnchor, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{ChannelConfig, ChannelUrl, PackageName};
use thiserror::Error;
use tracing::instrument;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    GetOutputDependenciesError, GetOutputDependenciesSpec, SourceCheckoutError,
    executor::ExecutorFutures,
};

/// A source package whose dependencies should be installed without building
/// the package itself. This is useful for development environments where you
/// want the dependencies of a package but not the package itself.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct DependencyOnlySource {
    /// The source specification (path/git/url)
    pub source: SourceSpec,

    /// The name of the output to extract dependencies from
    pub output_name: PackageName,
}

/// Result of expanding dev_sources into their dependencies.
#[derive(Debug, Clone, Default)]
pub struct ExpandedDevSources {
    /// All dependencies (build, host, and run) extracted from dev_sources
    pub dependencies: DependencyMap<PackageName, PixiSpec>,

    /// All constraints (build, host, and run) extracted from dev_sources
    pub constraints: DependencyMap<PackageName, BinarySpec>,
}

/// A specification for expanding dev sources into their dependencies.
///
/// Dev sources are source packages whose dependencies should be installed
/// without building the packages themselves. This is useful for development
/// environments where you want to work on a package while having its
/// dependencies available.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExpandDevSourcesSpec {
    /// The list of dev sources to expand
    pub dev_sources: Vec<DependencyOnlySource>,

    /// The channel configuration to use
    pub channel_config: ChannelConfig,

    /// The channels to use for solving
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Information about the build environment
    pub build_environment: BuildEnvironment,

    /// Variant configuration
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The protocols that are enabled for source packages
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

/// An error that can occur while expanding dev sources.
#[derive(Debug, Error, Diagnostic)]
pub enum ExpandDevSourcesError {
    #[error("failed to checkout source for package '{name}'")]
    SourceCheckout {
        name: String,
        #[source]
        #[diagnostic_source]
        error: SourceCheckoutError,
    },

    #[error("failed to get output dependencies for package '{}'", .name.as_source())]
    GetOutputDependencies {
        name: PackageName,
        #[source]
        #[diagnostic_source]
        error: GetOutputDependenciesError,
    },
}

impl ExpandDevSourcesSpec {
    /// Expands dev_sources into their dependencies.
    ///
    /// When a dev_source has a dependency on another package that is also
    /// in dev_sources, that dependency is not added to the result (since it
    /// will be processed separately).
    #[instrument(
        skip_all,
        name = "expand-dev-sources",
        fields(
            count = self.dev_sources.len(),
            platform = %self.build_environment.host_platform,
        )
    )]
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<ExpandedDevSources, CommandDispatcherError<ExpandDevSourcesError>> {
        // Create a lookup set for dev_sources by output_name
        // We'll use this to skip dependencies that are also dev_sources
        // TODO: In the future, we might want to also match by source location for more
        // precision
        let dev_source_names: HashSet<_> = self
            .dev_sources
            .iter()
            .map(|ds| ds.output_name.clone())
            .collect();

        // Process each dev_source concurrently
        let mut futures = ExecutorFutures::new(command_dispatcher.executor());

        for dev_source in self.dev_sources {
            futures.push(process_single_dev_source(
                dev_source,
                &command_dispatcher,
                &self.channel_config,
                &self.channels,
                &self.build_environment,
                &self.variants,
                &self.enabled_protocols,
                &dev_source_names,
            ));
        }

        // Collect results as they complete
        let mut result = ExpandedDevSources::default();
        while let Some(dev_source_result) = futures.next().await {
            let (dependencies, constraints) = dev_source_result?;

            // Merge dependencies and constraints into the result
            for (name, spec) in dependencies.into_specs() {
                result.dependencies.insert(name, spec);
            }
            for (name, spec) in constraints.into_specs() {
                result.constraints.insert(name, spec);
            }
        }

        Ok(result)
    }
}

/// Process a single dev_source: checkout, get dependencies, and filter/resolve them.
async fn process_single_dev_source(
    dev_source: DependencyOnlySource,
    command_dispatcher: &CommandDispatcher,
    channel_config: &ChannelConfig,
    channels: &[ChannelUrl],
    build_environment: &BuildEnvironment,
    variants: &Option<BTreeMap<String, Vec<String>>>,
    enabled_protocols: &EnabledProtocols,
    dev_source_names: &HashSet<PackageName>,
) -> Result<
    (
        DependencyMap<PackageName, PixiSpec>,
        DependencyMap<PackageName, BinarySpec>,
    ),
    CommandDispatcherError<ExpandDevSourcesError>,
> {
    // Pin and checkout the source
    let pinned_source = command_dispatcher
        .pin_and_checkout(dev_source.source.clone())
        .await
        .map_err_with(|error| ExpandDevSourcesError::SourceCheckout {
            name: dev_source.output_name.as_source().to_string(),
            error,
        })?;

    // Create a SourceAnchor for resolving relative paths in dependencies
    let source_anchor = SourceAnchor::from(SourceSpec::from(pinned_source.pinned.clone()));

    // Get the output dependencies
    let spec = GetOutputDependenciesSpec {
        source: pinned_source.pinned,
        output_name: dev_source.output_name.clone(),
        channel_config: channel_config.clone(),
        channels: channels.to_vec(),
        build_environment: build_environment.clone(),
        variants: variants.clone(),
        enabled_protocols: enabled_protocols.clone(),
    };

    let output_deps = command_dispatcher
        .get_output_dependencies(spec)
        .await
        .map_err_with(|error| ExpandDevSourcesError::GetOutputDependencies {
            name: dev_source.output_name.clone(),
            error,
        })?;

    // Process dependencies
    let mut dependencies = DependencyMap::default();
    let process_deps =
        |deps: Option<DependencyMap<PackageName, PixiSpec>>,
         dependencies: &mut DependencyMap<PackageName, PixiSpec>| {
            if let Some(deps) = deps {
                for (name, spec) in deps.into_specs() {
                    // Skip dependencies that are also dev_sources
                    // TODO: Currently matching by name only. In the future, we might want to
                    // also check if the source location matches for more precise matching.
                    if dev_source_names.contains(&name) {
                        continue;
                    }

                    // Resolve relative paths for source dependencies
                    let resolved_spec = match spec.into_source_or_binary() {
                        Either::Left(source) => {
                            // Resolve the source relative to the dev_source's location
                            PixiSpec::from(source_anchor.resolve(source))
                        }
                        Either::Right(binary) => {
                            // Binary specs don't need path resolution
                            PixiSpec::from(binary)
                        }
                    };
                    dependencies.insert(name, resolved_spec);
                }
            }
        };

    // Process all dependency types
    process_deps(output_deps.build_dependencies, &mut dependencies);
    process_deps(output_deps.host_dependencies, &mut dependencies);
    process_deps(Some(output_deps.run_dependencies), &mut dependencies);

    // Collect constraints
    let mut constraints = DependencyMap::default();
    if let Some(build_constraints) = output_deps.build_constraints {
        for (name, spec) in build_constraints.into_specs() {
            constraints.insert(name, spec);
        }
    }
    if let Some(host_constraints) = output_deps.host_constraints {
        for (name, spec) in host_constraints.into_specs() {
            constraints.insert(name, spec);
        }
    }
    for (name, spec) in output_deps.run_constraints.into_specs() {
        constraints.insert(name, spec);
    }

    Ok((dependencies, constraints))
}
