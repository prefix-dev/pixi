use std::{
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

use derive_more::Display;
use rattler_conda_types::{PackageName, Platform};

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
    /// lookup when the lockfile schema grows those slices); it does
    /// NOT affect the structural derivation today.
    Derived {
        parent: DerivedParent,
        package: PackageName,
        kind: DerivedEnvKind,
    },

    /// One-off spec that does not live in the registry. Content-hashed;
    /// used for per-call overrides like satisfiability's per-source
    /// `exclude_newer`.
    Ephemeral(Arc<EphemeralEnv>),
}

impl EnvironmentRef {
    /// Construct a [`Derived`](EnvironmentRef::Derived) env rooted in
    /// `self`. When `self` is already `Derived`, the chain is flattened
    /// by inheriting the inner `parent` and taking the outer `kind`.
    /// This is correct because the kind transforms compose trivially on
    /// `build_environment`: `Host` is the identity (clones the parent's
    /// build_environment) and `Build` is absorbing (`to_build_from_build`
    /// folds any parent down to build). Other spec fields pass through
    /// unchanged.
    pub fn derived(&self, package: PackageName, derived_env_kind: DerivedEnvKind) -> Self {
        match self {
            EnvironmentRef::Workspace(w) => Self::Derived {
                parent: DerivedParent::Workspace(w.clone()),
                package,
                kind: derived_env_kind,
            },
            EnvironmentRef::Derived { parent, .. } => Self::Derived {
                parent: parent.clone(),
                package,
                kind: derived_env_kind,
            },
            EnvironmentRef::Ephemeral(eph) => Self::Derived {
                parent: DerivedParent::Ephemeral(eph.clone()),
                package,
                kind: derived_env_kind,
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
            EnvironmentRef::Derived { parent, kind, .. } => {
                let parent_spec = parent.resolve(registry);
                let build_environment = match kind {
                    DerivedEnvKind::Build => parent_spec.build_environment.to_build_from_build(),
                    DerivedEnvKind::Host => parent_spec.build_environment.clone(),
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
    pub fn display_platform(&self) -> Platform {
        match self {
            EnvironmentRef::Workspace(ws) => ws.platform(),
            EnvironmentRef::Derived { parent, .. } => parent.display_platform(),
            EnvironmentRef::Ephemeral(eph) => eph.spec.build_environment.host_platform,
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
    pub fn display_platform(&self) -> Platform {
        match self {
            DerivedParent::Workspace(ws) => ws.platform(),
            DerivedParent::Ephemeral(eph) => eph.spec.build_environment.host_platform,
        }
    }
}

/// Which derived environment a [`EnvironmentRef::Derived`] represents.
/// The build/host derivation is a pure function of the parent
/// [`BuildEnvironment`](crate::BuildEnvironment).
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, Display)]
pub enum DerivedEnvKind {
    /// Environment used to run the build backend itself. Derived via
    /// [`BuildEnvironment::to_build_from_build`](crate::BuildEnvironment::to_build_from_build).
    Build,

    /// Environment the built package targets. Today this is a clone of
    /// the parent `BuildEnvironment`.
    Host,
}

impl fmt::Display for EnvironmentRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvironmentRef::Workspace(ws) => write!(f, "{ws}"),
            EnvironmentRef::Derived {
                parent,
                package,
                kind,
            } => write!(f, "{kind} of {} in {}", package.as_normalized(), parent),
            EnvironmentRef::Ephemeral(eph) => write!(
                f,
                "{}@{}",
                &eph.name, eph.spec.build_environment.host_platform
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
                &eph.name, eph.spec.build_environment.host_platform
            ),
        }
    }
}
