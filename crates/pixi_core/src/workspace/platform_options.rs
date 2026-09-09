//! Telling the user which other platforms an installed environment could use.
//!
//! The set of platforms that run on a machine grows over time: a teammate adds
//! one to the manifest, or the machine gains a capability such as a CUDA driver.
//! [`Environment::pinned_or_best_declared_platform`] keeps an installed
//! environment where it is, so those options go unnoticed unless we say so.
//!
//! We name each platform at most once per environment, and never rank them --
//! which platform suits the user is not something pixi can know.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use itertools::Itertools;
use pixi_consts::consts;
use pixi_manifest::PixiPlatform;
use serde::{Deserialize, Serialize};

use crate::workspace::{Environment, HasWorkspaceRef};

/// The platforms an environment could be installed for on this machine,
/// relative to the one it is installed for now.
#[derive(Debug)]
pub struct AlternativePlatforms<'p> {
    /// The platform the environment is installed for, as recorded in
    /// `conda-meta/pixi`.
    pub installed: &'p PixiPlatform,

    /// Platforms that run here and that the environment declares, excluding
    /// `installed`, in [`Environment::runnable_declared_platforms`] order --
    /// an order of consideration, not a recommendation.
    pub alternatives: Vec<&'p PixiPlatform>,
}

/// The platforms `environment` could be installed for besides the one it is
/// installed for now.
///
/// `None` when it isn't installed, when the platform it was installed for is no
/// longer declared or no longer runs here, or when nothing else runs here.
pub fn alternative_platforms<'p>(
    environment: &Environment<'p>,
) -> Option<AlternativePlatforms<'p>> {
    let installed = environment.installed_resolved_platform()?;
    let runnable = environment.runnable_declared_platforms();

    // A lapsed pin means the environment is about to move whatever the user
    // wants, so offering alternatives would imply it still has a choice.
    if !runnable.contains(&installed) {
        return None;
    }

    let alternatives: Vec<&'p PixiPlatform> = runnable
        .into_iter()
        .filter(|platform| *platform != installed)
        .collect();

    (!alternatives.is_empty()).then_some(AlternativePlatforms {
        installed,
        alternatives,
    })
}

/// Which alternative platforms we have already told the user about for one
/// environment of one workspace.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReportedPlatformOptions {
    /// The platform installed when we last looked. Informational, so the file
    /// explains itself to whoever asks why a message did or didn't appear.
    installed: String,

    /// Every platform this environment's user has been shown or has used. A
    /// platform in here is never mentioned again.
    known: Vec<String>,
}

impl ReportedPlatformOptions {
    /// The alternatives the user hasn't met yet, in the order given, plus the
    /// state to persist. An empty first element means stay quiet.
    ///
    /// `known` only ever grows and counts the installed platform, so a platform
    /// that comes and goes, or that a `--platform` switch moved away from, is
    /// named once and not again.
    fn plan_report(
        stored: Option<Self>,
        installed: &str,
        alternatives: &[&str],
    ) -> (Vec<String>, Self) {
        let mut state = stored.unwrap_or_default();
        state.installed = installed.to_string();

        let unreported: Vec<String> = alternatives
            .iter()
            .filter(|name| !state.known.iter().any(|seen| seen == *name))
            .map(ToString::to_string)
            .collect();

        // The platform in use is known by definition; without this, switching
        // would make the platform left behind look like a fresh discovery.
        state.known.push(installed.to_string());
        state.known.extend(unreported.iter().cloned());
        state.known.sort_unstable();
        state.known.dedup();
        (unreported, state)
    }
}

/// State files we have already handled in this process, so a multi-task
/// `pixi run` reports at most once. Mirrors the dedup in
/// [`crate::workspace::virtual_packages`].
static REPORTED_IN_PROCESS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Report that this machine can run platforms other than the one `environment`
/// is installed for, at most once per process and once per platform per
/// workspace.
///
/// Best-effort: an unreadable state file counts as nothing reported yet, and one
/// we cannot write only means the message may repeat.
pub fn report_new_platform_options(environment: &Environment<'_>) {
    let state_path = state_file_path(environment);

    let Ok(mut handled) = REPORTED_IN_PROCESS.lock() else {
        return;
    };
    if !handled.insert(state_path.clone()) {
        return;
    }
    drop(handled);

    let Some(options) = alternative_platforms(environment) else {
        return;
    };

    let installed = options.installed.name().as_str();
    let alternatives: Vec<&str> = options
        .alternatives
        .iter()
        .map(|platform| platform.name().as_str())
        .collect();
    let stored = read_state(&state_path);
    let (unreported, state) =
        ReportedPlatformOptions::plan_report(stored.clone(), installed, &alternatives);

    // Persist on any change, not just when reporting, so the file keeps saying
    // what we last observed. The common case -- no change -- writes nothing.
    if stored.as_ref() != Some(&state) {
        write_state(&state_path, &state);
    }

    if unreported.is_empty() {
        return;
    }

    // Naming one platform would read as a recommendation, so only do it when
    // there is nothing to choose between.
    let switch_target = match unreported.as_slice() {
        [only] => only.as_str(),
        _ => "<name>",
    };
    tracing::warn!(
        "Environment {} is installed for platform {}. This machine can also run {}. \
         pixi keeps using {} until you switch it: `pixi install -e {} --platform {}`. \
         Run `pixi workspace platform list` to see what each platform declares.",
        consts::ENVIRONMENT_STYLE.apply_to(environment.name().as_str()),
        consts::PLATFORM_STYLE.apply_to(installed),
        unreported
            .iter()
            .map(|name| consts::PLATFORM_STYLE.apply_to(name.as_str()).to_string())
            .format(", "),
        consts::PLATFORM_STYLE.apply_to(installed),
        environment.name().as_str(),
        switch_target,
    );
}

/// Where the reported-options state for `environment` lives, alongside the
/// workspace's other one-time messages. `pixi clean` removes `.pixi/envs` but
/// not this directory, so a reinstall re-announces nothing; delete the file to
/// hear the message again.
fn state_file_path(environment: &Environment<'_>) -> PathBuf {
    environment
        .workspace()
        .pixi_dir()
        .join(consts::ONE_TIME_MESSAGES_DIR)
        .join(format!("platform-options-{}.json", environment.name()))
}

/// `None` when the file is absent or unreadable; a corrupt file is discarded so
/// a stale format can't silence the message forever.
fn read_state(path: &Path) -> Option<ReportedPlatformOptions> {
    let contents = fs_err::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn write_state(path: &Path, state: &ReportedPlatformOptions) {
    let Ok(contents) = serde_json::to_string_pretty(state) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(err) = fs_err::create_dir_all(parent)
        .and_then(|()| pixi_utils::atomic_write::atomic_write_sync(path, contents))
    {
        tracing::debug!(
            "failed to record reported platform options in '{}': {err}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the sequence a user actually sees: a new option is announced,
    /// repeats stay quiet, a later option is announced on its own, and an
    /// option that disappears and returns is not announced again.
    #[test]
    fn each_option_is_reported_once() {
        let (reported, state) =
            ReportedPlatformOptions::plan_report(None, "linux-64", &["x86-cuda13"]);
        assert_eq!(reported, ["x86-cuda13"]);

        let (reported, state) =
            ReportedPlatformOptions::plan_report(Some(state), "linux-64", &["x86-cuda13"]);
        assert!(reported.is_empty(), "a repeat run must stay quiet");

        let (reported, state) = ReportedPlatformOptions::plan_report(
            Some(state),
            "linux-64",
            &["x86-cuda13", "x86-v3"],
        );
        assert_eq!(reported, ["x86-v3"], "only the option that is new");

        // The machine loses CUDA, then regains it.
        let (reported, state) =
            ReportedPlatformOptions::plan_report(Some(state), "linux-64", &["x86-v3"]);
        assert!(reported.is_empty());
        let (reported, _) = ReportedPlatformOptions::plan_report(
            Some(state),
            "linux-64",
            &["x86-cuda13", "x86-v3"],
        );
        assert!(
            reported.is_empty(),
            "an option the user already saw must not come back"
        );
    }

    /// Switching to one of the alternatives says nothing new: the user has been
    /// told which platforms run here, and hearing the same set from the other
    /// side is the nagging the once-per-option rule exists to prevent.
    #[test]
    fn switching_the_installed_platform_is_not_news() {
        let (reported, state) =
            ReportedPlatformOptions::plan_report(None, "linux-64", &["x86-cuda13", "x86-v3"]);
        assert_eq!(reported, ["x86-cuda13", "x86-v3"]);

        // The user switches to x86-cuda13; the alternatives are now linux-64
        // (where they came from) and x86-v3 (already announced above).
        let (reported, state) = ReportedPlatformOptions::plan_report(
            Some(state),
            "x86-cuda13",
            &["linux-64", "x86-v3"],
        );
        assert!(
            reported.is_empty(),
            "no platform in this workspace is still unannounced"
        );
        assert_eq!(state.installed, "x86-cuda13", "the pin is still tracked");

        // A genuinely new platform is still announced after a switch.
        let (reported, _) = ReportedPlatformOptions::plan_report(
            Some(state),
            "x86-cuda13",
            &["linux-64", "x86-v3", "x86-v4"],
        );
        assert_eq!(reported, ["x86-v4"]);
    }
}
