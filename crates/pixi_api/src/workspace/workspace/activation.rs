use indexmap::IndexMap;
use miette::IntoDiagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::{FeatureName, HasWorkspaceManifest, TargetSelector};

use crate::Interface;

/// The activation content of a single feature/target combination in the
/// manifest.
#[derive(Debug, Clone)]
pub struct ActivationEntry {
    /// The feature the activation belongs to.
    pub feature: FeatureName,
    /// The target selector the activation belongs to, or `None` for the
    /// default target.
    pub target: Option<TargetSelector>,
    /// The activation scripts, in execution order.
    pub scripts: Vec<String>,
    /// The activation environment variables.
    pub env: IndexMap<String, String>,
}

/// Lists every non-empty activation table in the manifest, in manifest order.
pub async fn list(workspace: &Workspace) -> Vec<ActivationEntry> {
    let mut entries = Vec::new();
    for (name, feature) in workspace.workspace_manifest().all_features() {
        for (target_data, selector) in feature.targets.iter() {
            let Some(activation) = &target_data.activation else {
                continue;
            };
            let scripts = activation.scripts.clone().unwrap_or_default();
            let env = activation.env.clone().unwrap_or_default();
            if scripts.is_empty() && env.is_empty() {
                continue;
            }
            entries.push(ActivationEntry {
                feature: name.clone(),
                target: selector.cloned(),
                scripts,
                env,
            });
        }
    }
    entries
}

/// Adds activation scripts to the manifest and saves it. With `prepend` the
/// scripts are placed (or moved) to the front of the list.
pub async fn add_scripts<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    scripts: Vec<String>,
    prepend: bool,
    target: Option<TargetSelector>,
    feature_name: FeatureName,
) -> miette::Result<()> {
    let warnings = script_warnings(workspace.workspace(), &scripts, target.as_ref());

    let change = workspace.manifest().add_activation_scripts(
        scripts,
        prepend,
        target.as_ref(),
        &feature_name,
    )?;

    workspace.save().await.into_diagnostic()?;

    let location = location_suffix(target.as_ref(), &feature_name);
    for script in &change.added {
        interface
            .success(&format!("Added activation script '{script}'{location}"))
            .await;
    }
    for script in &change.already_present {
        if prepend {
            interface
                .success(&format!(
                    "Moved activation script '{script}' to the front{location}"
                ))
                .await;
        } else {
            interface
                .success(&format!(
                    "Activation script '{script}' is already present{location}; nothing to do"
                ))
                .await;
        }
    }
    for warning in warnings {
        interface.warning(&warning).await;
    }

    Ok(())
}

/// Removes activation scripts from the manifest and saves it.
pub async fn remove_scripts<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    scripts: Vec<String>,
    target: Option<TargetSelector>,
    feature_name: FeatureName,
) -> miette::Result<()> {
    workspace.manifest().remove_activation_scripts(
        scripts.clone(),
        target.as_ref(),
        &feature_name,
    )?;

    workspace.save().await.into_diagnostic()?;

    let location = location_suffix(target.as_ref(), &feature_name);
    for script in &scripts {
        interface
            .success(&format!("Removed activation script '{script}'{location}"))
            .await;
    }

    Ok(())
}

/// Sets (inserts or overwrites) activation environment variables in the
/// manifest and saves it.
pub async fn set_env<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    variables: Vec<(String, String)>,
    target: Option<TargetSelector>,
    feature_name: FeatureName,
) -> miette::Result<()> {
    workspace
        .manifest()
        .set_activation_env(variables.clone(), target.as_ref(), &feature_name)?;

    workspace.save().await.into_diagnostic()?;

    let location = location_suffix(target.as_ref(), &feature_name);
    for (key, value) in &variables {
        interface
            .success(&format!(
                "Set activation environment variable {key}=\"{value}\"{location}"
            ))
            .await;
    }

    Ok(())
}

/// Removes activation environment variables from the manifest and saves it.
pub async fn remove_env<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    keys: Vec<String>,
    target: Option<TargetSelector>,
    feature_name: FeatureName,
) -> miette::Result<()> {
    workspace
        .manifest()
        .remove_activation_env(keys.clone(), target.as_ref(), &feature_name)?;

    workspace.save().await.into_diagnostic()?;

    let location = location_suffix(target.as_ref(), &feature_name);
    for key in &keys {
        interface
            .success(&format!(
                "Removed activation environment variable '{key}'{location}"
            ))
            .await;
    }

    Ok(())
}

/// A suffix describing the feature/target an edit applied to, e.g.
/// ` in feature "cuda" for target 'linux-64'`. Empty for the default
/// feature/target.
fn location_suffix(target: Option<&TargetSelector>, feature_name: &FeatureName) -> String {
    let mut suffix = String::new();
    if !feature_name.is_default() {
        suffix.push_str(&format!(" in {}", feature_name.user_facing()));
    }
    if let Some(target) = target {
        suffix.push_str(&format!(" for target '{target}'"));
    }
    suffix
}

/// Best-effort warnings for newly added scripts: a script that does not exist
/// on disk (yet), or a shell script that will run on platforms whose
/// activation shell cannot execute it.
fn script_warnings(
    workspace: &Workspace,
    scripts: &[String],
    target: Option<&TargetSelector>,
) -> Vec<String> {
    let mut warnings = Vec::new();

    // The platforms the scripts will run on: workspace platforms matching the
    // target selector (or all of them when no target is given).
    let platforms = &workspace.workspace_manifest().workspace.platforms;
    let runs_on_windows = platforms
        .iter()
        .filter(|platform| target.is_none_or(|target| target.matches(platform)))
        .any(|platform| platform.subdir().is_windows());
    let runs_on_unix = platforms
        .iter()
        .filter(|platform| target.is_none_or(|target| target.matches(platform)))
        .any(|platform| platform.subdir().is_unix());

    for script in scripts {
        if !workspace.root().join(script).is_file() {
            warnings.push(format!(
                "The activation script '{script}' does not exist (yet); it is resolved relative to the workspace root"
            ));
        }
        if script.ends_with(".sh") && runs_on_windows {
            warnings.push(format!(
                "The activation script '{script}' will also run on Windows, where activation uses `cmd.exe`; consider limiting it with `--target unix`"
            ));
        }
        if script.ends_with(".bat") && runs_on_unix {
            warnings.push(format!(
                "The activation script '{script}' will also run on Unix platforms, where activation uses `bash`; consider limiting it with `--target win`"
            ));
        }
    }

    warnings
}
