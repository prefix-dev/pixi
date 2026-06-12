use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use miette::Diagnostic;
use pixi_build_types::{InputGlobSet, ProjectModel};
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage0::{
    About, ConditionalList, Item, License, Package, SingleOutputRecipe, Value,
};
use rattler_build_recipe::stage1;
use rattler_build_types::NormalizedKey;
use rattler_conda_types::{
    ChannelUrl, PackageName, Platform, SourcePackageName, Version, VersionWithSource,
};
use serde::de::DeserializeOwned;
use thiserror::Error;
use url::Url;

use crate::specs_conversion::{
    SelectorConversionError, from_targets_v1_to_conditional_requirements,
};

#[derive(Debug, Clone, Default)]
pub struct PythonParams {
    // Returns whether the build is editable or not.
    // Default to false
    pub editable: bool,
}

/// The trait is responsible of converting a certain [`ProjectModel`] (or
/// others in the future) into a [`SingleOutputRecipe`].
/// By implementing this trait, you can create a new backend for `pixi-build`.
///
/// It also uses a [`BackendConfig`] to provide additional configuration
/// options.
///
///
/// An instance of this trait is used by the [`crate::intermediate_backend::IntermediateBackend`]
/// in order to generate the recipe.
#[async_trait::async_trait]
pub trait GenerateRecipe {
    type Config: BackendConfig;

    /// Generates a [`SingleOutputRecipe`] from a [`ProjectModel`].
    ///
    /// # Parameters
    ///
    /// * `model` - The project model to convert into a recipe
    /// * `config` - Backend-specific configuration options
    /// * `manifest_path` - Path to the project manifest file
    /// * `host_platform` - The host platform will be removed in the future.
    ///   Right now it is used to determine if certain dependencies are present
    ///   for the host platform. Instead, we should rely on recipe selectors and
    ///   offload all the evaluation logic to the rattler-build.
    /// * `python_params` - Used only by python backend right now and may
    ///   be removed when profiles will be implemented.
    /// * `variants` - The variant names that are available to the recipe. This might
    ///   influence how the recipe is generated.
    /// * `channels` - The channels that are being used for this build. This can be
    ///   used for backend-specific logic that depends on which channels are available.
    /// * `cache_dir` - Optional cache directory for storing cached data (e.g., HTTP responses).
    /// * `workspace_scratch_directory` - Optional per-workspace scratch directory the backend
    ///   may use to persist derived state across runs and across multiple backend instances.
    ///   The backend picks its own subdirectory inside and owns invalidation. See
    ///   `pixi_build_types::procedures::initialize::InitializeParams::workspace_scratch_directory`
    ///   for the convention.
    /// * `workspace_directory` - Absolute path to the root of the workspace that owns this
    ///   package, if any. Backends can use it to inspect sibling packages (e.g. the ROS
    ///   backend uses it to discover sibling `package.xml` files and emit them as source
    ///   dependencies). `None` when the package is built outside of a workspace context.
    /// * `checkout_root` - Absolute path to the root of this package's source checkout.
    ///   For a git or url source dependency this is the directory pixi unpacked the
    ///   checkout into, BEFORE any `subdirectory` is applied — distinct from
    ///   `workspace_directory` (a pixi-workspace concept) and from `manifest_path`
    ///   (the package's own dir). Backends that need to reason about siblings inside
    ///   the same checkout (e.g. ROS workspace sibling-package discovery) anchor their
    ///   search here when no pixi workspace is available.
    #[allow(clippy::too_many_arguments)]
    async fn generate_recipe(
        &self,
        model: &ProjectModel,
        config: &Self::Config,
        manifest_path: PathBuf,
        host_platform: Platform,
        python_params: Option<PythonParams>,
        variants: &HashSet<NormalizedKey>,
        channels: Vec<ChannelUrl>,
        cache_dir: Option<PathBuf>,
        workspace_scratch_directory: Option<PathBuf>,
        workspace_directory: Option<PathBuf>,
        checkout_root: Option<PathBuf>,
    ) -> miette::Result<GeneratedRecipe>;

    /// Called during `conda/build/v1` after the recipe has been rendered and
    /// right before the build is executed.
    ///
    /// `rendered_recipe` carries the concrete requirements after rattler-build
    /// evaluated all `if(...)` conditions, so this is the only place where a
    /// backend can base build script decisions on conditional dependencies;
    /// [`Self::generate_recipe`] only sees the default target. Returning
    /// `Some(content)` replaces the build script content of the rendered
    /// recipe, returning `None` keeps the script from
    /// [`Self::generate_recipe`].
    ///
    /// `generated_recipe` is mutable so implementations can also amend the
    /// input globs based on the rendered requirements.
    fn finalize_build_script(
        &self,
        _rendered_recipe: &stage1::Recipe,
        _generated_recipe: &mut GeneratedRecipe,
        _config: &Self::Config,
        _manifest_path: &Path,
        _host_platform: Platform,
        _python_params: Option<PythonParams>,
    ) -> miette::Result<Option<String>> {
        Ok(None)
    }

    /// Returns a list of globs that should be used to find the input files
    /// for the build process.
    /// For example, this could be a list of source files or configuration files
    /// used by Cmake.
    fn extract_input_globs_from_build(
        &self,
        _config: &Self::Config,
        _workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// Returns "default" variants for the given host platform. This allows
    /// backends to set some default variant configuration that can be
    /// completely overwritten by the user.
    ///
    /// This can be useful to change the default behavior of rattler-build with
    /// regard to compilers. But it also allows setting up default build
    /// matrices.
    fn default_variants(
        &self,
        _host_platform: Platform,
    ) -> miette::Result<BTreeMap<NormalizedKey, Vec<Variable>>> {
        Ok(BTreeMap::new())
    }
}

pub trait BackendConfig: DeserializeOwned + Clone {
    /// Debug dir provided by the backend config
    fn debug_dir(&self) -> Option<&Path>;

    /// Merge this configuration with a target-specific configuration.
    /// Target-specific values typically override base values.
    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self>;
}

#[derive(Debug, Error, Diagnostic)]
pub enum GenerateRecipeError<MetadataProviderError: Diagnostic + 'static> {
    #[error("There was no name defined for the recipe")]
    NoNameDefined,
    #[error("There was no version defined for the recipe")]
    NoVersionDefined,
    #[error("failed to parse package name")]
    InvalidPackageName(
        #[source]
        #[from]
        rattler_conda_types::InvalidPackageNameError,
    ),
    #[error("failed to parse version")]
    InvalidVersion(String),
    #[error("failed to parse URL: {0}")]
    InvalidUrl(String),
    #[error("failed to parse license: {0}")]
    InvalidLicense(String),
    #[error("An error occurred while querying the {0}")]
    MetadataProviderError(
        String,
        #[diagnostic_source]
        #[source]
        MetadataProviderError,
    ),
    #[error(transparent)]
    #[diagnostic(transparent)]
    InvalidSelectorExpression(#[from] SelectorConversionError),
}

#[derive(Clone)]
pub struct GeneratedRecipe {
    pub recipe: SingleOutputRecipe,
    /// Globs whose matched files contribute to the metadata-cache fingerprint.
    /// Pixi evaluates them with gitignore "last match wins" semantics, so
    /// backends MUST emit inclusion patterns before any `!`-prefixed
    /// exclusions that should override them.
    pub metadata_input_globs: Vec<String>,
    /// Optional structured form of [`Self::metadata_input_globs`].  Backends
    /// that can describe their inputs precisely (e.g. workspace discovery
    /// with markers) populate this; pixi prefers it over the flat list when
    /// it is non-empty and falls back otherwise.
    pub metadata_input_glob_sets: Vec<InputGlobSet>,
    /// Globs whose matched files trigger a rebuild. Same ordering rule as
    /// [`Self::metadata_input_globs`].
    pub build_input_globs: Vec<String>,
    /// Optional structured form of [`Self::build_input_globs`].  See
    /// [`Self::metadata_input_glob_sets`] for semantics.
    pub build_input_glob_sets: Vec<InputGlobSet>,
}

/// Returns the package names of the concrete match spec dependencies in a
/// rendered requirement list. Pin dependencies and specs without an exact
/// name are skipped.
pub fn rendered_dependency_names(
    dependencies: &[stage1::Dependency],
) -> impl Iterator<Item = &PackageName> {
    dependencies
        .iter()
        .filter_map(|dependency| match dependency {
            stage1::Dependency::Spec(spec) => spec.name.as_exact(),
            _ => None,
        })
}

/// Helper to create a concrete `Value<Url>` from an optional string
fn parse_url_value(s: String) -> Option<Value<Url>> {
    match Url::parse(&s) {
        Ok(url) => Some(Value::new_concrete(url, None)),
        Err(err) => {
            tracing::warn!("failed to parse URL '{s}': {err}");
            None
        }
    }
}

/// Helper to create a concrete `Value<License>` from an optional string
fn parse_license_value(s: String) -> Option<Value<License>> {
    match License::from_str(&s) {
        Ok(lic) => Some(Value::new_concrete(lic, None)),
        Err(err) => {
            tracing::warn!("failed to parse license '{s}': {err}");
            None
        }
    }
}

impl GeneratedRecipe {
    /// Creates a new [`GeneratedRecipe`] from a [`ProjectModel`].
    /// A default implementation that doesn't take into account the
    /// build scripts or other fields.
    pub fn from_model<M: MetadataProvider>(
        model: ProjectModel,
        provider: &mut M,
    ) -> Result<Self, GenerateRecipeError<M::Error>> {
        // If the name is not defined in the model, we try to get it from the provider.
        // If the provider cannot provide a name, we return an error.
        let name = match model.name {
            Some(name) => {
                if name.trim().is_empty() {
                    return Err(GenerateRecipeError::NoNameDefined);
                } else {
                    name
                }
            }
            None => provider
                .name()
                .map_err(|e| GenerateRecipeError::MetadataProviderError(String::from("name"), e))?
                .ok_or(GenerateRecipeError::NoNameDefined)?,
        };

        // Recipes only allow lowercase names
        let name = name.to_lowercase();

        // If the version is not defined in the model, we try to get it from the
        // provider. If the provider cannot provide a version, we return an
        // error.
        let version = match model.version {
            Some(v) => v,
            None => provider
                .version()
                .map_err(|e| {
                    GenerateRecipeError::MetadataProviderError(String::from("version"), e)
                })?
                .ok_or(GenerateRecipeError::NoVersionDefined)?,
        };

        let pkg_name = rattler_conda_types::PackageName::try_from(name)?;
        let version_str = version.to_string();
        let version_with_source = VersionWithSource::from_str(&version_str)
            .map_err(|_| GenerateRecipeError::InvalidVersion(version_str))?;

        let package = Package::new(
            Value::new_concrete(SourcePackageName::from(pkg_name), None),
            Value::new_concrete(version_with_source, None),
        );

        let requirements =
            from_targets_v1_to_conditional_requirements(&model.targets.unwrap_or_default())?;

        macro_rules! derive_value {
            ($ident:ident) => {
                match model.$ident {
                    Some(v) => Some(v.to_string()),
                    None => provider.$ident().map_err(|e| {
                        GenerateRecipeError::MetadataProviderError(
                            String::from(stringify!($ident)),
                            e,
                        )
                    })?,
                }
            };
        }

        let about = About {
            homepage: derive_value!(homepage).and_then(parse_url_value),
            license: derive_value!(license).and_then(parse_license_value),
            description: derive_value!(description).map(|s| Value::new_concrete(s, None)),
            documentation: derive_value!(documentation).and_then(parse_url_value),
            repository: derive_value!(repository).and_then(parse_url_value),
            license_file: match model.license_file {
                Some(v) => {
                    let item = Item::Value(Value::new_concrete(v.display().to_string(), None));
                    Some(ConditionalList::new(vec![item]))
                }
                None => provider
                    .license_files()
                    .map_err(|e| {
                        GenerateRecipeError::MetadataProviderError(String::from("license-files"), e)
                    })?
                    .map(|files| {
                        ConditionalList::new(
                            files
                                .into_iter()
                                .map(|f| Item::Value(Value::new_concrete(f, None)))
                                .collect(),
                        )
                    }),
            },
            license_family: None,
            summary: provider
                .summary()
                .map_err(|e| {
                    GenerateRecipeError::MetadataProviderError(String::from("summary"), e)
                })?
                .map(|s| Value::new_concrete(s, None)),
        };

        let mut recipe = SingleOutputRecipe::new(package);
        recipe.requirements = requirements;
        if let Some(flags) = model.build_flags {
            recipe.build.flags = ConditionalList::new(
                flags
                    .into_iter()
                    .map(|flag| Item::Value(Value::new_concrete(flag, None)))
                    .collect(),
            );
        }
        recipe.about = about;

        Ok(GeneratedRecipe {
            recipe,
            metadata_input_globs: Vec::new(),
            metadata_input_glob_sets: Vec::new(),
            build_input_globs: Vec::new(),
            build_input_glob_sets: Vec::new(),
        })
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum MetadataProviderError {
    #[error("The metadata provider cannot provide an about section for the recipe")]
    CannotParseVersion(#[from] rattler_conda_types::ParseVersionError),
}

pub trait MetadataProvider {
    type Error: Diagnostic;

    /// Returns the name of the package or `None` if the provider does not
    /// provide a name.
    fn name(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }

    /// Returns the version of the package or `None` if the provider does not
    /// provide a version.
    fn version(&mut self) -> Result<Option<Version>, Self::Error> {
        Ok(None)
    }

    fn homepage(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
    fn license(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
    fn license_files(&mut self) -> Result<Option<Vec<String>>, Self::Error> {
        Ok(None)
    }
    fn summary(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
    fn description(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
    fn documentation(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
    fn repository(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
}

pub struct DefaultMetadataProvider;

impl MetadataProvider for DefaultMetadataProvider {
    type Error = Infallible;
}

#[cfg(test)]
mod tests {
    use ordermap::OrderMap;
    use pixi_build_types::{
        BinaryPackageSpec, ConditionalExpression, PackageSpec, SourcePackageName, Target, Targets,
    };
    use rattler_conda_types::{Flag, PackageName};

    use super::*;

    fn extras_with_gtest()
    -> OrderMap<pixi_build_types::ExtraGroupName, OrderMap<SourcePackageName, PackageSpec>> {
        let mut dependencies = OrderMap::new();
        dependencies.insert(
            SourcePackageName::from(PackageName::new_unchecked("gtest")),
            BinaryPackageSpec {
                version: Some("*".parse().unwrap()),
                ..BinaryPackageSpec::default()
            }
            .into(),
        );
        let mut extras = OrderMap::new();
        extras.insert(
            pixi_build_types::ExtraGroupName::new("test").unwrap(),
            dependencies,
        );
        extras
    }

    #[test]
    fn generated_recipe_declares_package_extras() {
        let model = ProjectModel {
            name: Some("example".to_string()),
            version: Some("0.1.0".parse().unwrap()),
            targets: Some(Targets {
                default_target: Some(Target {
                    extra_dependencies: Some(extras_with_gtest()),
                    ..Target::default()
                }),
                conditional: None,
            }),
            ..ProjectModel::default()
        };

        let generated = GeneratedRecipe::from_model(model, &mut DefaultMetadataProvider).unwrap();
        let value = serde_json::to_value(&generated.recipe.requirements.extras).unwrap();

        assert!(crate::v3::generated_recipe_uses_v3(&generated.recipe));
        assert_eq!(
            value,
            serde_json::json!({
                "test": ["gtest"]
            })
        );
    }

    /// Conditional extras must be wrapped in a `Conditional` block in the
    /// generated recipe rather than landing as a bare entry.
    #[test]
    fn generated_recipe_declares_per_target_extras() {
        let mut conditional_targets = OrderMap::new();
        conditional_targets.insert(
            ConditionalExpression::new("win"),
            Target {
                extra_dependencies: Some(extras_with_gtest()),
                ..Target::default()
            },
        );

        let model = ProjectModel {
            name: Some("example".to_string()),
            version: Some("0.1.0".parse().unwrap()),
            targets: Some(Targets {
                default_target: None,
                conditional: Some(conditional_targets),
            }),
            ..ProjectModel::default()
        };

        let generated = GeneratedRecipe::from_model(model, &mut DefaultMetadataProvider).unwrap();
        let test_group = generated
            .recipe
            .requirements
            .extras
            .get("test")
            .expect("test group present");
        let first = test_group
            .iter()
            .next()
            .expect("test group has at least one item");
        assert!(
            matches!(first, rattler_build_recipe::stage0::Item::Conditional(_)),
            "per-target extras must be wrapped in a Conditional in the generated recipe",
        );
        assert!(crate::v3::generated_recipe_uses_v3(&generated.recipe));
    }

    #[test]
    fn generated_recipe_declares_build_flags() {
        let model = ProjectModel {
            name: Some("example".to_string()),
            version: Some("0.1.0".parse().unwrap()),
            build_flags: Some(vec![
                "cuda".parse::<Flag>().unwrap(),
                "blas_openblas".parse::<Flag>().unwrap(),
            ]),
            ..ProjectModel::default()
        };

        let generated = GeneratedRecipe::from_model(model, &mut DefaultMetadataProvider).unwrap();
        let value = serde_json::to_value(&generated.recipe.build.flags).unwrap();

        assert!(crate::v3::generated_recipe_uses_v3(&generated.recipe));
        assert_eq!(value, serde_json::json!(["cuda", "blas_openblas"]));
    }

    #[test]
    fn generated_recipe_without_v3_features_does_not_require_v3() {
        let model = ProjectModel {
            name: Some("example".to_string()),
            version: Some("0.1.0".parse().unwrap()),
            ..ProjectModel::default()
        };

        let generated = GeneratedRecipe::from_model(model, &mut DefaultMetadataProvider).unwrap();

        assert!(!crate::v3::generated_recipe_uses_v3(&generated.recipe));
    }
}
