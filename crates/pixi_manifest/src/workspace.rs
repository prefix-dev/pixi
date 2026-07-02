use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use indexmap::{IndexMap, IndexSet};
use pixi_pypi_spec::PypiPackageName;
use pixi_spec::{ExcludeNewer, TomlSpec};
use pixi_toml::TomlEnum;
use rattler_conda_types::{
    Arch, GenericVirtualPackage, NamedChannelOrUrl, PackageName, Platform, Version, VersionSpec,
};
use serde::Deserialize;
use toml_span::{DeserError, Value};
use url::Url;

use super::pypi::pypi_options::PypiOptions;
use crate::{
    PixiPlatform, PixiPlatformName, PrioritizedChannel, S3Options, TargetSelector, Targets,
    preview::Preview,
};
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use once_cell::sync::Lazy;

pub static JINJA_ENV: Lazy<Environment<'static>> = Lazy::new(|| {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_auto_escape_callback(|_| AutoEscape::None);
    env
});

/// Describes the contents of the `[workspace]` section of the project manifest.
#[derive(Debug, Default, Clone)]
pub struct Workspace {
    /// The name of the project
    pub name: Option<String>,

    /// The version of the project
    pub version: Option<Version>,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    pub authors: Option<Vec<String>>,

    /// The channels used by the project
    pub channels: IndexSet<PrioritizedChannel>,

    /// Channel priority for the whole project
    pub channel_priority: Option<ChannelPriority>,

    /// Solve strategy for the whole project.
    pub solve_strategy: Option<SolveStrategy>,

    /// The platforms this project supports
    pub platforms: IndexSet<PixiPlatform>,

    /// The license as a valid SPDX string (e.g. MIT AND Apache-2.0)
    pub license: Option<String>,

    /// The license file (relative to the project root)
    pub license_file: Option<PathBuf>,

    /// Path to the README file of the project (relative to the project root)
    pub readme: Option<PathBuf>,

    /// URL of the project homepage
    pub homepage: Option<Url>,

    /// URL of the project source repository
    pub repository: Option<Url>,

    /// URL of the project documentation
    pub documentation: Option<Url>,

    /// The conda to pypi name mapping configuration.
    pub conda_pypi_map: Option<CondaPypiMap>,

    /// The pypi options supported in the project
    pub pypi_options: Option<PypiOptions>,

    /// The S3 options supported in the project
    pub s3_options: Option<HashMap<String, S3Options>>,

    /// Preview features
    pub preview: Preview,

    /// Build variants defined directly in the manifest.
    pub build_variants: Targets<Option<HashMap<String, Vec<String>>>>,

    /// Ordered list of external variant configuration files.
    pub build_variant_files: Vec<BuildVariantSource>,

    /// Version requirement for pixi itself
    pub requires_pixi: Option<VersionSpec>,

    /// Exclude package candidates that are newer than this date.
    pub exclude_newer: Option<ExcludeNewer>,

    /// Workspace-wide conda package exclude-newer overrides.
    pub exclude_newer_package_overrides: IndexMap<PackageName, ExcludeNewer>,

    /// Workspace-wide PyPI package exclude-newer overrides.
    pub pypi_exclude_newer_package_overrides: IndexMap<PypiPackageName, ExcludeNewer>,

    /// `[workspace.dependencies]` pool. Path specs remain relative to
    /// `root_directory`; members re-base them at inheritance time.
    pub dependencies: IndexMap<PackageName, TomlSpec>,

    /// Absolute directory of the workspace manifest. Used to re-base relative
    /// path specs in `dependencies` for members in other directories.
    pub root_directory: PathBuf,

    /// Set during parsing when the source pixi.toml uses the legacy
    /// `[system-requirements]` shape on top of subdir-only platforms. The
    /// next add/edit operation that produces a non-subdir platform persists
    /// the in-memory migration to disk so the file moves to the new syntax.
    pub must_migrate: bool,

    /// Set during parsing when the declared platforms were all plain subdirs,
    /// so environments combine the per-feature rich platforms that share a
    /// subdir into one platform carrying the union of their virtual packages.
    /// Cleared for workspaces that declare custom rich platforms, which are
    /// matched by name instead.
    pub use_platform_composition: bool,
}

impl Workspace {
    /// Look up a configured [`PixiPlatform`] by its name.
    pub fn platform_by_name(&self, name: &PixiPlatformName) -> Option<&PixiPlatform> {
        self.platforms.iter().find(|p| p.name() == name)
    }

    /// Returns the [`TargetSelector`] used to key the target table for a
    /// platform name, matching how the platform is declared in the workspace
    /// (`Subdir` for bare subdir platforms, `Platform` for richer ones).
    pub fn target_selector_for_platform(&self, name: &PixiPlatformName) -> TargetSelector {
        self.platform_by_name(name)
            .map(PixiPlatform::as_target_selector)
            .unwrap_or_else(|| TargetSelector::Platform(name.clone()))
    }

    /// Return every workspace [`PixiPlatform`] whose subdir matches `current`
    /// or one of the fallback subdirs used by
    /// `Environment::best_platform_with_current`, ordered from most to least
    /// appropriate. Within each subdir bucket, platforms are returned in
    /// workspace declaration order (so a custom-named variant declared after
    /// the bare subdir-bound platform comes second).
    ///
    /// Platforms whose declared virtual packages are not satisfied by
    /// `system_virtual_packages` are filtered out -- e.g. a `__cuda`-requiring
    /// platform is dropped on a system that does not provide CUDA.
    pub fn possible_pixi_platforms(
        &self,
        current: Platform,
        system_virtual_packages: &[GenericVirtualPackage],
    ) -> Vec<&PixiPlatform> {
        let candidate_subdirs = self.candidate_subdirs(current);

        // Subdir-default virtual packages are pixi's assumed baseline for
        // the target subdir, not a host requirement -- a `win-64` entry's
        // materialised `__win` would otherwise rule out matching when this
        // process happens to run on linux (cross-platform `pixi info`,
        // CI on a different host, etc.). Only the user-customised VPs
        // need to be satisfied by the host.
        let satisfies_system = |p: &&PixiPlatform| {
            p.declared_virtual_packages()
                .iter()
                .filter(|declared| !crate::platform::is_subdir_default(declared, p.subdir()))
                .all(|declared| satisfied_by_system(declared, system_virtual_packages))
        };

        let mut result: Vec<&PixiPlatform> = Vec::new();
        for subdir in &candidate_subdirs {
            result.extend(
                self.platforms
                    .iter()
                    .filter(|p| p.subdir() == *subdir)
                    .filter(satisfies_system),
            );
        }

        // Single-workspace-platform WASM fallback, mirroring
        // `best_platform_with_current`.
        if self.platforms.len() == 1
            && let Some(p) = self.platforms.iter().next()
            && p.subdir().arch() == Some(Arch::Wasm32)
            && !candidate_subdirs.contains(&p.subdir())
            && satisfies_system(&p)
        {
            result.push(p);
        }

        result
    }

    /// Subdirs pixi will consider when matching the host platform: `current`
    /// plus the same architecture fallbacks used by
    /// [`Self::possible_pixi_platforms`].
    pub fn candidate_subdirs(&self, current: Platform) -> Vec<Platform> {
        let mut candidate_subdirs: Vec<Platform> = vec![current];
        if current.is_osx() && current != Platform::Osx64 {
            candidate_subdirs.push(Platform::Osx64);
        }
        if current.is_windows() && current != Platform::Win64 {
            candidate_subdirs.push(Platform::Win64);
        }
        if current == Platform::Win64 {
            candidate_subdirs.push(Platform::Win32);
        }
        candidate_subdirs
    }

    /// Declared virtual packages from `env_platforms` whose host subdir
    /// matches `current` but whose requirement is not provided by
    /// `system_virtual_packages`. Powers the
    /// [`Self::possible_pixi_platforms`]-returns-nothing diagnostic, so the
    /// caller can tell the user which VPs to mock via `CONDA_OVERRIDE_*`.
    pub fn unsatisfied_platform_requirements(
        &self,
        current: Platform,
        system_virtual_packages: &[GenericVirtualPackage],
        env_platforms: &HashSet<PixiPlatformName>,
    ) -> Vec<GenericVirtualPackage> {
        let mut unsatisfied: Vec<GenericVirtualPackage> = Vec::new();
        for diagnosis in self
            .platform_match_diagnostics(current, system_virtual_packages, env_platforms)
            .into_iter()
            .filter(|d| d.subdir_matches_host)
        {
            for declared in diagnosis.unsatisfied_virtual_packages {
                if !unsatisfied
                    .iter()
                    .any(|u| u.name == declared.name && u.version == declared.version)
                {
                    unsatisfied.push(declared);
                }
            }
        }
        unsatisfied
    }

    /// Explain, for each platform an environment declares, why it does or
    /// does not run on the current host: whether its subdir is runnable here
    /// and which declared virtual packages the host fails to provide. Subdir
    /// defaults never count as unsatisfied (they're pixi's baseline, not a
    /// host requirement, see [`Self::possible_pixi_platforms`]).
    ///
    /// Platforms are returned in workspace declaration order.
    pub fn platform_match_diagnostics(
        &self,
        current: Platform,
        system_virtual_packages: &[GenericVirtualPackage],
        env_platforms: &HashSet<PixiPlatformName>,
    ) -> Vec<PlatformMatchDiagnosis> {
        let candidate_subdirs = self.candidate_subdirs(current);
        self.platforms
            .iter()
            .filter(|p| env_platforms.contains(p.name()))
            .map(|p| {
                let subdir = p.subdir();
                let unsatisfied_virtual_packages = p
                    .declared_virtual_packages()
                    .iter()
                    .filter(|declared| !crate::platform::is_subdir_default(declared, subdir))
                    .filter(|declared| !satisfied_by_system(declared, system_virtual_packages))
                    .cloned()
                    .collect();
                PlatformMatchDiagnosis {
                    name: p.name().clone(),
                    subdir,
                    subdir_matches_host: candidate_subdirs.contains(&subdir),
                    unsatisfied_virtual_packages,
                }
            })
            .collect()
    }
}

/// Why a single declared platform does or does not run on the current host,
/// produced by [`Workspace::platform_match_diagnostics`].
#[derive(Debug, Clone)]
pub struct PlatformMatchDiagnosis {
    /// The declared platform's name.
    pub name: PixiPlatformName,

    /// The conda subdir the platform targets.
    pub subdir: Platform,

    /// Whether `subdir` is one the current host can run (its own subdir or an
    /// architecture fallback such as `win-64` → `win-32`).
    pub subdir_matches_host: bool,

    /// Declared virtual packages (excluding subdir defaults) the host does not
    /// provide at a high enough version. Empty when the only mismatch is the
    /// subdir, or when the platform runs here.
    pub unsatisfied_virtual_packages: Vec<GenericVirtualPackage>,
}

impl PlatformMatchDiagnosis {
    /// `true` when this platform runs on the current host: its subdir matches
    /// and every declared virtual package is satisfied.
    pub fn matches_host(&self) -> bool {
        self.subdir_matches_host && self.unsatisfied_virtual_packages.is_empty()
    }
}

/// Returns true if `declared` is provided by the system: the system must list
/// a virtual package of the same name with a version at least as high as the
/// declared one.
fn satisfied_by_system(declared: &GenericVirtualPackage, system: &[GenericVirtualPackage]) -> bool {
    system
        .iter()
        .find(|s| s.name == declared.name)
        .is_some_and(|s| s.version >= declared.version)
}

/// A source that contributes additional build variant definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildVariantSource {
    /// Load variants from a file relative to the workspace root.
    File(PathBuf),
}

#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    strum::Display,
    strum::VariantNames,
    strum::EnumString,
    Deserialize,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ChannelPriority {
    #[default]
    Strict,
    Disabled,
}

impl<'de> toml_span::Deserialize<'de> for ChannelPriority {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlEnum::deserialize(value).map(TomlEnum::into_inner)
    }
}

impl From<ChannelPriority> for rattler_solve::ChannelPriority {
    fn from(value: ChannelPriority) -> Self {
        match value {
            ChannelPriority::Strict => rattler_solve::ChannelPriority::Strict,
            ChannelPriority::Disabled => rattler_solve::ChannelPriority::Disabled,
        }
    }
}

impl From<rattler_solve::ChannelPriority> for ChannelPriority {
    fn from(value: rattler_solve::ChannelPriority) -> Self {
        match value {
            rattler_solve::ChannelPriority::Strict => ChannelPriority::Strict,
            rattler_solve::ChannelPriority::Disabled => ChannelPriority::Disabled,
        }
    }
}

/// The value of `[workspace.conda-pypi-map]`.
#[derive(Debug, Clone, PartialEq)]
pub enum CondaPypiMap {
    /// `conda-pypi-map = false`: disable purl derivation entirely, including
    /// the offline same-name heuristic.
    Disabled,
    /// Per-channel mapping configuration. An empty map is a soft-deprecated
    /// alias for `Disabled`.
    Map(HashMap<NamedChannelOrUrl, CondaPypiMapEntry>),
}

/// How a project-defined channel mapping interacts with the default
/// prefix.dev derivation chain.
#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    strum::Display,
    strum::VariantNames,
    strum::EnumString,
)]
#[strum(serialize_all = "kebab-case")]
pub enum CondaPypiMappingMode {
    /// The project mapping overlays Pixi's default mapping data: project
    /// entries win, and misses fall through to the prefix.dev chain.
    #[default]
    Overlay,
    /// The project mapping replaces Pixi's default mapping data. The
    /// same-name heuristic is controlled separately.
    Replace,
}

/// The mapping configuration for one channel in `[workspace.conda-pypi-map]`.
#[derive(Debug, Clone, PartialEq)]
pub enum CondaPypiMapEntry {
    /// `<channel> = false`: disable purl derivation for this channel.
    Disabled,
    /// A mapping defined by a location (file or URL) and/or inline entries.
    Map(CondaPypiMapSpec),
}

/// A channel mapping built from up to two sources: an external location and
/// inline entries. Inline entries override entries from the location.
#[derive(Debug, Clone, PartialEq)]
pub struct CondaPypiMapSpec {
    /// An external mapping JSON file: a file path or http(s) URL. Unresolved:
    /// relative paths are resolved against the workspace root by the consumer.
    pub location: Option<String>,
    /// Inline conda-name to pypi-name entries. One conda package may map to
    /// several PyPI names. An empty list (spelled `false` in TOML) means the
    /// package is not a PyPI package.
    pub mapping: Option<HashMap<String, Vec<String>>>,
    pub mapping_mode: CondaPypiMappingMode,
    /// Whether Pixi may assume the conda package name is also the PyPI name
    /// when mapping data has no answer. If unset, this defaults to true for
    /// conda-forge and false for other channels.
    pub same_name_heuristic: Option<bool>,
}

impl CondaPypiMapEntry {
    /// Create an entry from a bare location string. Bare strings use the
    /// default (overlay) mapping mode.
    pub fn from_location(location: String) -> Self {
        Self::Map(CondaPypiMapSpec {
            location: Some(location),
            mapping: None,
            mapping_mode: CondaPypiMappingMode::default(),
            same_name_heuristic: None,
        })
    }
}

#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    strum::Display,
    strum::VariantNames,
    strum::EnumString,
    Deserialize,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SolveStrategy {
    #[default]
    Highest,
    Lowest,
    LowestDirect,
}

impl<'de> toml_span::Deserialize<'de> for SolveStrategy {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlEnum::deserialize(value).map(TomlEnum::into_inner)
    }
}

impl From<SolveStrategy> for rattler_solve::SolveStrategy {
    fn from(value: SolveStrategy) -> Self {
        match value {
            SolveStrategy::Highest => rattler_solve::SolveStrategy::Highest,
            SolveStrategy::Lowest => rattler_solve::SolveStrategy::LowestVersion,
            SolveStrategy::LowestDirect => rattler_solve::SolveStrategy::LowestVersionDirect,
        }
    }
}

impl From<rattler_solve::SolveStrategy> for SolveStrategy {
    fn from(value: rattler_solve::SolveStrategy) -> Self {
        match value {
            rattler_solve::SolveStrategy::Highest => Self::Highest,
            rattler_solve::SolveStrategy::LowestVersion => Self::Lowest,
            rattler_solve::SolveStrategy::LowestVersionDirect => Self::LowestDirect,
        }
    }
}
