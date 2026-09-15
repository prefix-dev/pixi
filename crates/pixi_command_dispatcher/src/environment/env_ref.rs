use std::{
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

use derive_more::Display;
use rattler_conda_types::PackageName;

use super::{EnvironmentSpec, WorkspaceEnvRef, WorkspaceEnvRegistry};

/// Reference to an environment input bundle that a compute depends on.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum EnvironmentRef {
    /// The common case. Content lives in the registry; identity is the
    /// registry id, which is stable across the dispatcher's lifetime.
    Workspace(WorkspaceEnvRef),

    /// A structural transform of a parent env, used for the build/host
    /// environments that nested source solves need. The `package`
    /// field is carried for future use (per-package installed-hints
    /// lookup when the lock file schema grows those slices); it does
    /// NOT affect the structural derivation today.
    Derived {
        parent: DerivedParent,
        package: PackageName,
        /// The relation of this environment to `package`: its build or
        /// its host environment. Used for labels (traces, cycle frames).
        kind: DerivedEnvKind,
        /// Which of the parent's platforms this environment is solved
        /// for. This is what [`EnvironmentRef::resolve`] applies.
        platform: DerivedPlatform,
    },

    /// One-off spec that does not live in the registry. Content-hashed;
    /// used for per-call overrides like satisfiability's per-source
    /// `exclude_newer`.
    Ephemeral(Arc<EphemeralEnv>),
}

impl EnvironmentRef {
    /// Construct a [`Derived`](EnvironmentRef::Derived) env rooted in
    /// `self`.
    ///
    /// When `self` is already `Derived`, the chain is flattened: the new
    /// ref inherits the inner `parent`, takes the outer `kind` as its
    /// label, and composes the platform transforms. `Host` is the identity
    /// on the parent's `build_environment` and `Build` is absorbing
    /// (`to_build_from_build` folds any parent down to the build
    /// platform), so the composed [`DerivedPlatform`] is `Build` as soon
    /// as any derivation in the chain was a `Build` derivation: a package
    /// solved inside a build environment runs on the build platform, and
    /// so do its own build and host dependencies. Other spec fields pass
    /// through unchanged.
    pub fn derived(&self, package: PackageName, derived_env_kind: DerivedEnvKind) -> Self {
        match self {
            EnvironmentRef::Workspace(w) => Self::Derived {
                parent: DerivedParent::Workspace(w.clone()),
                package,
                kind: derived_env_kind,
                platform: DerivedPlatform::of_kind(derived_env_kind),
            },
            EnvironmentRef::Derived {
                parent, platform, ..
            } => Self::Derived {
                parent: parent.clone(),
                package,
                kind: derived_env_kind,
                platform: match platform {
                    DerivedPlatform::Build => DerivedPlatform::Build,
                    DerivedPlatform::Host => DerivedPlatform::of_kind(derived_env_kind),
                },
            },
            EnvironmentRef::Ephemeral(eph) => Self::Derived {
                parent: DerivedParent::Ephemeral(eph.clone()),
                package,
                kind: derived_env_kind,
                platform: DerivedPlatform::of_kind(derived_env_kind),
            },
        }
    }
}

/// Parent of a [`EnvironmentRef::Derived`]. Non-recursive by
/// construction so the type cannot express a Derived-of-Derived chain
/// and [`EnvironmentRef::resolve`] cannot recurse unboundedly. See
/// [`EnvironmentRef::derived`] for why the flatten it performs is
/// correct.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum DerivedParent {
    /// The parent is a registered workspace env; resolving goes through
    /// the registry.
    Workspace(WorkspaceEnvRef),

    /// The parent is an inline, content-hashed spec that does not live
    /// in the registry.
    Ephemeral(Arc<EphemeralEnv>),
}

/// Inline environment spec outside the
/// [`WorkspaceEnvRegistry`]. `name` is
/// display-only and excluded from identity; content-hashed on `spec`.
#[derive(Debug, Clone)]
pub struct EphemeralEnv {
    pub name: String,
    pub spec: Arc<EnvironmentSpec>,
}

impl EphemeralEnv {
    pub fn new(name: impl Into<String>, spec: EnvironmentSpec) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            spec: Arc::new(spec),
        })
    }
}

impl Hash for EphemeralEnv {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Destructure so adding a field below forces a decision about
        // its hash contribution. `name` is display-only, excluded from
        // identity.
        let Self { name: _, spec } = self;
        spec.hash(state);
    }
}

impl PartialEq for EphemeralEnv {
    fn eq(&self, other: &Self) -> bool {
        self.spec == other.spec
    }
}

impl Eq for EphemeralEnv {}

impl EnvironmentRef {
    /// Resolve this ref to the underlying [`EnvironmentSpec`], applying
    /// any `Derived` transform. Prefer the projection Keys from inside
    /// a compute body so the engine tracks the dependency; this helper
    /// is for call sites outside compute bodies.
    // TODO(baszalmstra): Remove this once everything is migrated to the compute engine.
    pub fn resolve(&self, registry: &WorkspaceEnvRegistry) -> Arc<EnvironmentSpec> {
        match self {
            EnvironmentRef::Workspace(ws) => registry.get(ws.id()),
            EnvironmentRef::Derived {
                parent, platform, ..
            } => {
                let parent_spec = parent.resolve(registry);
                let build_environment = match platform {
                    DerivedPlatform::Build => parent_spec.build_environment.to_build_from_build(),
                    DerivedPlatform::Host => parent_spec.build_environment.clone(),
                };
                Arc::new(EnvironmentSpec {
                    build_environment,
                    ..(*parent_spec).clone()
                })
            }
            EnvironmentRef::Ephemeral(eph) => eph.spec.clone(),
        }
    }

    /// Platform label without registry access; for logging/formatting
    /// from contexts that don't hold the registry. If the registry is
    /// available, prefer [`resolve`](Self::resolve).
    pub fn display_platform(&self) -> String {
        match self {
            EnvironmentRef::Workspace(ws) => ws.platform().to_string(),
            EnvironmentRef::Derived { parent, .. } => parent.display_platform(),
            EnvironmentRef::Ephemeral(eph) => eph.spec.build_environment.host_platform.to_string(),
        }
    }
}

impl DerivedParent {
    /// Resolve this parent to its underlying [`EnvironmentSpec`]. Used
    /// by [`EnvironmentRef::resolve`] and projections when walking
    /// through a Derived env.
    pub fn resolve(&self, registry: &WorkspaceEnvRegistry) -> Arc<EnvironmentSpec> {
        match self {
            DerivedParent::Workspace(ws) => registry.get(ws.id()),
            DerivedParent::Ephemeral(eph) => eph.spec.clone(),
        }
    }

    /// Display-only platform for this parent, without registry access.
    pub fn display_platform(&self) -> String {
        match self {
            DerivedParent::Workspace(ws) => ws.platform().to_string(),
            DerivedParent::Ephemeral(eph) => eph.spec.build_environment.host_platform.to_string(),
        }
    }
}

/// The relation of a [`EnvironmentRef::Derived`] environment to its
/// package: the environment its build backend runs in, or the
/// environment the built package targets. Labels traces and cycle
/// frames; the platform the environment is solved for is
/// [`DerivedPlatform`].
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, Display)]
pub enum DerivedEnvKind {
    /// Environment used to run the build backend itself.
    Build,

    /// Environment the built package targets.
    Host,
}

/// Which of the parent's two platforms a [`EnvironmentRef::Derived`]
/// environment is solved for.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub enum DerivedPlatform {
    /// The parent's build platform: the parent's `BuildEnvironment` is
    /// folded with
    /// [`BuildEnvironment::to_build_from_build`](crate::BuildEnvironment::to_build_from_build).
    Build,

    /// The parent's host platform: the parent's `BuildEnvironment` is
    /// used unchanged.
    Host,
}

impl DerivedPlatform {
    /// The platform a `kind` environment is solved for when its parent is
    /// solved for the parent's own host platform: the build environment
    /// runs on the build platform, the host environment targets the host
    /// platform.
    pub fn of_kind(kind: DerivedEnvKind) -> Self {
        match kind {
            DerivedEnvKind::Build => DerivedPlatform::Build,
            DerivedEnvKind::Host => DerivedPlatform::Host,
        }
    }
}

impl fmt::Display for EnvironmentRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvironmentRef::Workspace(ws) => write!(f, "{ws}"),
            EnvironmentRef::Derived {
                parent,
                package,
                kind,
                ..
            } => write!(f, "{kind} of {} in {}", package.as_normalized(), parent),
            EnvironmentRef::Ephemeral(eph) => write!(
                f,
                "{}@{}",
                eph.name, eph.spec.build_environment.host_platform
            ),
        }
    }
}

impl fmt::Display for DerivedParent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DerivedParent::Workspace(ws) => write!(f, "{ws}"),
            DerivedParent::Ephemeral(eph) => write!(
                f,
                "{}@{}",
                eph.name, eph.spec.build_environment.host_platform
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rattler_conda_types::{GenericVirtualPackage, Platform};
    use rattler_solve::ChannelPriority;

    use pixi_utils::variants::VariantConfig;

    use super::*;
    use crate::{BuildEnvironment, environment::WorkspaceEnvRegistry};

    fn virtual_package(name: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: name.parse().unwrap(),
            version: "1".parse().unwrap(),
            build_string: "0".into(),
        }
    }

    /// A cross-compiling parent: host and build differ in platform and in
    /// virtual packages, so a swapped transform is observable.
    fn cross_parent() -> (EnvironmentRef, BuildEnvironment) {
        let build_environment = BuildEnvironment {
            host_platform: Platform::OsxArm64,
            host_virtual_packages: vec![virtual_package("__osx")],
            build_platform: Platform::Linux64,
            build_virtual_packages: vec![virtual_package("__glibc")],
        };
        let env_ref = EnvironmentRef::Ephemeral(EphemeralEnv::new(
            "publish",
            EnvironmentSpec {
                channels: vec![],
                build_environment: build_environment.clone(),
                variants: VariantConfig::default(),
                exclude_newer: None,
                channel_priority: ChannelPriority::Strict,
            },
        ));
        (env_ref, build_environment)
    }

    fn resolve(env_ref: &EnvironmentRef) -> Arc<EnvironmentSpec> {
        // Ephemeral parents never touch the registry.
        env_ref.resolve(&WorkspaceEnvRegistry::new())
    }

    fn pkg(name: &str) -> PackageName {
        PackageName::new_unchecked(name)
    }

    #[test]
    fn build_then_host_stays_on_the_build_platform() {
        let (parent, parent_env) = cross_parent();
        let nested = parent
            .derived(pkg("a"), DerivedEnvKind::Build)
            .derived(pkg("b"), DerivedEnvKind::Host);

        assert_eq!(
            resolve(&nested).build_environment,
            parent_env.to_build_from_build(),
            "the host env of a build dependency is the build platform"
        );
        let EnvironmentRef::Derived { kind, platform, .. } = &nested else {
            panic!("expected a derived env");
        };
        assert_eq!(*kind, DerivedEnvKind::Host, "the label keeps the relation");
        assert_eq!(*platform, DerivedPlatform::Build);
    }

    #[test]
    fn host_then_build_moves_to_the_build_platform() {
        let (parent, parent_env) = cross_parent();
        let nested = parent
            .derived(pkg("a"), DerivedEnvKind::Host)
            .derived(pkg("b"), DerivedEnvKind::Build);
        assert_eq!(
            resolve(&nested).build_environment,
            parent_env.to_build_from_build()
        );
    }

    #[test]
    fn build_then_build_stays_on_the_build_platform() {
        let (parent, parent_env) = cross_parent();
        let nested = parent
            .derived(pkg("a"), DerivedEnvKind::Build)
            .derived(pkg("b"), DerivedEnvKind::Build);
        assert_eq!(
            resolve(&nested).build_environment,
            parent_env.to_build_from_build()
        );
    }

    #[test]
    fn host_then_host_keeps_the_parent_environment() {
        let (parent, parent_env) = cross_parent();
        let nested = parent
            .derived(pkg("a"), DerivedEnvKind::Host)
            .derived(pkg("b"), DerivedEnvKind::Host);
        assert_eq!(resolve(&nested).build_environment, parent_env);
    }

    #[test]
    fn refs_differing_only_in_platform_are_distinct_keys() {
        let (parent, _) = cross_parent();
        let via_build = parent
            .derived(pkg("a"), DerivedEnvKind::Build)
            .derived(pkg("b"), DerivedEnvKind::Host);
        let via_host = parent
            .derived(pkg("a"), DerivedEnvKind::Host)
            .derived(pkg("b"), DerivedEnvKind::Host);
        assert_ne!(via_build, via_host);
    }
}
