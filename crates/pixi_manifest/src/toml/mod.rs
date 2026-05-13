mod build_backend;
mod build_target;
mod channel;
mod document;
mod environment;
mod feature;
mod manifest;
mod package;
mod package_target;
mod platform;
mod preview;
mod pypi_options;
pub mod pyproject;
mod s3_options;
mod system_requirements;
mod target;
mod task;
mod workspace;

use std::{borrow::Cow, ops::Range};

pub use build_backend::{BackendSpec, TomlPackageBuild};
pub use channel::TomlPrioritizedChannel;
pub use document::TomlDocument;
pub use environment::{TomlEnvironment, TomlEnvironmentList};
pub use feature::TomlFeature;
use itertools::Itertools;
pub use manifest::ExternalWorkspaceProperties;
pub use manifest::TomlManifest;
use miette::LabeledSpan;
pub use package::{PackageDefaults, PackageError, TomlPackage, WorkspacePackageProperties};
pub use platform::TomlPlatform;
pub use preview::TomlPreview;
pub use pyproject::PyProjectToml;
pub use target::TomlTarget;
use toml_span::{DeserError, Span};
pub use workspace::TomlWorkspace;

use rattler_conda_types::Platform;

use crate::PixiPlatform;
use crate::{TargetSelector, TomlError, error::GenericError, utils::PixiSpanned};

pub trait FromTomlStr {
    fn from_toml_str(source: &str) -> Result<Self, TomlError>
    where
        Self: Sized;
}

impl<T: for<'de> toml_span::Deserialize<'de>> FromTomlStr for T {
    fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_span::parse(source)
            .map_err(DeserError::from)
            .and_then(|mut v| toml_span::Deserialize::deserialize(&mut v))
            .map_err(TomlError::from)
    }
}

/// An enum that contains a span to a `platforms =` section. Either from a
/// feature or a workspace.
enum PlatformSpan {
    Feature(String, Span),
    Workspace(Span),
}

fn create_unsupported_selector_warning(
    platform_span: PlatformSpan,
    selector: &PixiSpanned<TargetSelector>,
    matching_platforms: &[&PixiPlatform],
) -> GenericError {
    let (feature_or_workspace, span) = match platform_span {
        PlatformSpan::Feature(name, span) => (Cow::Owned(format!("feature '{name}'")), span),
        PlatformSpan::Workspace(span) => (Cow::Borrowed("workspace"), span),
    };

    // Build the suggestion list:
    //   1. workspace platforms whose name resolves under the selector (these are already declared,
    //      so they are the cheapest fix), and
    //   2. for family selectors (`unix`, `linux`, `win`, `osx`) the conda subdirs the family
    //      covers, so the user also sees not-yet-defined options they could add.
    let mut suggestions: Vec<String> = matching_platforms
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    if matches!(
        &selector.value,
        TargetSelector::Linux | TargetSelector::Unix | TargetSelector::Win | TargetSelector::MacOs
    ) {
        for subdir in Platform::all()
            .filter(|p| selector.value.matches(&PixiPlatform::from_subdir(*p)))
            .map(|p| p.to_string())
        {
            if !suggestions.contains(&subdir) {
                suggestions.push(subdir);
            }
        }
    }
    if suggestions.is_empty() {
        suggestions.push(selector.value.to_string());
    }

    GenericError::new(format!(
        "The target selector '{}' does not match any of the platforms supported by the {}",
        selector.value, feature_or_workspace,
    ))
    .with_opt_span(selector.span.clone())
    .with_span_label("target selector specified here")
    .with_label(LabeledSpan::new_with_span(
        Some(format!(
            "the platforms supported by the {feature_or_workspace} are defined here"
        )),
        Range::<usize>::from(span),
    ))
    .with_help(match suggestions.as_slice() {
        [single] => format!(
            "Add '{single}' to the supported platforms, using `pixi project platform add {single}`",
        ),
        many => format!(
            "Add one of {0} to the supported platforms, using `pixi project platform add {1}`",
            many.iter()
                .format_with(", ", |p, f| f(&format_args!("'{p}'"))),
            many[0],
        ),
    })
}
