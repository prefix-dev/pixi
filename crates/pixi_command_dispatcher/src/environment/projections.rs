use std::sync::Arc;

use derive_more::Display;
use pixi_compute_engine::{BuildEnvironment, ComputeCtx, Key};
use pixi_spec::ResolvedExcludeNewer;
use rattler_conda_types::ChannelUrl;

use pixi_utils::variants::VariantConfig;

use super::{EnvironmentRef, EnvironmentSpec, HasWorkspaceEnvRegistry};

/// Resolve an [`EnvironmentRef`] to the [`EnvironmentSpec`] behind it,
/// applying the `Derived` transform when present. Delegates to
/// [`EnvironmentRef::resolve`] with the engine's registry.
///
/// The registry read is not tracked in the dep graph; that's safe
/// because the registry is append-only and a given id maps to one
/// immutable spec forever.
fn resolve_spec(ctx: &ComputeCtx, env: &EnvironmentRef) -> Arc<EnvironmentSpec> {
    env.resolve(ctx.global_data().workspace_env_registry())
}

/// Projection of an [`EnvironmentRef`]'s channel list.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Display)]
#[display("{_0}")]
pub struct ChannelsOf(pub EnvironmentRef);

impl Key for ChannelsOf {
    type Value = Arc<Vec<ChannelUrl>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = resolve_spec(ctx, &self.0);
        Arc::new(spec.channels.clone())
    }
}

/// Projection of an [`EnvironmentRef`]'s [`BuildEnvironment`]. The
/// `Derived` transform is applied by [`EnvironmentRef::resolve`], so
/// this projection just reads `build_environment` off the resolved
/// spec.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Display)]
#[display("{_0}")]
pub struct BuildEnvOf(pub EnvironmentRef);

impl Key for BuildEnvOf {
    type Value = Arc<BuildEnvironment>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = resolve_spec(ctx, &self.0);
        Arc::new(spec.build_environment.clone())
    }
}

/// Projection of an [`EnvironmentRef`]'s variant configuration.
/// Bundles the inline map and the file list since consumers always
/// read both together.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Display)]
#[display("{_0}")]
pub struct VariantsOf(pub EnvironmentRef);

impl Key for VariantsOf {
    type Value = Arc<VariantConfig>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = resolve_spec(ctx, &self.0);
        Arc::new(spec.variants.clone())
    }
}

/// Projection of an [`EnvironmentRef`]'s `exclude_newer` cutoff.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Display)]
#[display("{_0}")]
pub struct ExcludeNewerOf(pub EnvironmentRef);

impl Key for ExcludeNewerOf {
    type Value = Arc<Option<ResolvedExcludeNewer>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = resolve_spec(ctx, &self.0);
        Arc::new(spec.exclude_newer.clone())
    }
}

#[cfg(test)]
mod tests {
    use pixi_compute_engine::ComputeEngine;
    use rattler_conda_types::{ChannelUrl, PackageName, Platform};
    use rattler_solve::ChannelPriority;

    use pixi_compute_engine::BuildEnvironment;

    use super::*;
    use crate::environment::{
        DerivedEnvKind, DerivedParent, EnvironmentSpec, WorkspaceEnvRegistry,
    };

    fn channel(url: &str) -> ChannelUrl {
        ChannelUrl::from(url::Url::parse(url).expect("valid url"))
    }

    fn spec_with_channels(channels: Vec<ChannelUrl>) -> EnvironmentSpec {
        EnvironmentSpec {
            channels,
            build_environment: BuildEnvironment {
                host_platform: Platform::Linux64,
                host_virtual_packages: Vec::new(),
                build_platform: Platform::Linux64,
                build_virtual_packages: Vec::new(),
            },
            variants: VariantConfig::default(),
            exclude_newer: None,
            channel_priority: ChannelPriority::Strict,
        }
    }

    /// Build an engine with just the registry injected, so tests can
    /// exercise projection compute bodies without constructing a full
    /// `CommandDispatcher`.
    fn engine_with_registry(
        registry: Arc<WorkspaceEnvRegistry>,
    ) -> (ComputeEngine, Arc<WorkspaceEnvRegistry>) {
        let engine = ComputeEngine::builder().with_data(registry.clone()).build();
        (engine, registry)
    }

    #[tokio::test]
    async fn channels_of_workspace_returns_registered_channels() {
        let (engine, registry) = engine_with_registry(Arc::new(WorkspaceEnvRegistry::new()));
        let ws = registry.allocate(
            "default".to_string(),
            Platform::Linux64,
            spec_with_channels(vec![channel("https://example.com/conda-forge/")]),
        );

        let channels = engine
            .compute(&ChannelsOf(EnvironmentRef::Workspace(ws)))
            .await
            .expect("compute must succeed");

        assert_eq!(
            channels.as_slice(),
            &[channel("https://example.com/conda-forge/")]
        );
    }

    #[tokio::test]
    async fn channels_of_distinct_workspaces_route_to_each_spec() {
        let (engine, registry) = engine_with_registry(Arc::new(WorkspaceEnvRegistry::new()));
        let ws_a = registry.allocate(
            "a".to_string(),
            Platform::Linux64,
            spec_with_channels(vec![channel("https://example.com/a/")]),
        );
        let ws_b = registry.allocate(
            "b".to_string(),
            Platform::Linux64,
            spec_with_channels(vec![channel("https://example.com/b/")]),
        );

        let a = engine
            .compute(&ChannelsOf(EnvironmentRef::Workspace(ws_a)))
            .await
            .unwrap();
        let b = engine
            .compute(&ChannelsOf(EnvironmentRef::Workspace(ws_b)))
            .await
            .unwrap();

        assert_eq!(a.as_slice(), &[channel("https://example.com/a/")]);
        assert_eq!(b.as_slice(), &[channel("https://example.com/b/")]);
    }

    #[tokio::test]
    async fn channels_of_derived_inherits_from_parent() {
        let (engine, registry) = engine_with_registry(Arc::new(WorkspaceEnvRegistry::new()));
        let parent = registry.allocate(
            "default".to_string(),
            Platform::Linux64,
            spec_with_channels(vec![channel("https://example.com/parent/")]),
        );

        let derived = EnvironmentRef::Derived {
            parent: DerivedParent::Workspace(parent),
            package: PackageName::new_unchecked("foo"),
            kind: DerivedEnvKind::Build,
        };

        let channels = engine.compute(&ChannelsOf(derived)).await.unwrap();
        assert_eq!(
            channels.as_slice(),
            &[channel("https://example.com/parent/")]
        );
    }

    #[tokio::test]
    async fn build_env_of_derived_build_applies_build_from_build_transform() {
        let (engine, registry) = engine_with_registry(Arc::new(WorkspaceEnvRegistry::new()));
        let parent_build_env = BuildEnvironment {
            host_platform: Platform::Linux64,
            host_virtual_packages: Vec::new(),
            build_platform: Platform::OsxArm64,
            build_virtual_packages: Vec::new(),
        };
        let mut spec = spec_with_channels(vec![]);
        spec.build_environment = parent_build_env.clone();
        let parent = registry.allocate("default".to_string(), Platform::Linux64, spec);

        let derived_build = engine
            .compute(&BuildEnvOf(EnvironmentRef::Derived {
                parent: DerivedParent::Workspace(parent.clone()),
                package: PackageName::new_unchecked("foo"),
                kind: DerivedEnvKind::Build,
            }))
            .await
            .unwrap();
        assert_eq!(
            *derived_build,
            parent_build_env.to_build_from_build(),
            "Derived::Build must apply to_build_from_build()"
        );

        let derived_host = engine
            .compute(&BuildEnvOf(EnvironmentRef::Derived {
                parent: DerivedParent::Workspace(parent),
                package: PackageName::new_unchecked("foo"),
                kind: DerivedEnvKind::Host,
            }))
            .await
            .unwrap();
        assert_eq!(
            *derived_host, parent_build_env,
            "Derived::Host must clone the parent build_environment"
        );
    }
}
