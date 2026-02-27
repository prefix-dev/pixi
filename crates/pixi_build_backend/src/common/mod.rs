//! Common utilities that are shared between the different build backends.
mod configuration;
mod requirements;
mod variants;

pub use configuration::{BuildConfigurationParams, build_configuration};
pub use requirements::{PackageRequirements, SourceRequirements, requirements};
pub use variants::compute_variants;

/// Determine whether build output should include ANSI color codes.
///
/// Checks standard color environment variables in precedence order:
///   1. `FORCE_COLOR` (any value) → true
///   2. `CLICOLOR_FORCE` (non-"0" value) → true
///   3. `NO_COLOR` (any value) → false
///   4. `CLICOLOR=0` → false
///   5. Otherwise → false (backend stderr is piped, not a TTY)
///
/// When the pixi frontend spawns a build backend it sets `FORCE_COLOR=1` or
/// `NO_COLOR=1` based on the user's resolved preference, so this function
/// automatically picks up `--color`, `PIXI_COLOR`, and all the standard
/// variables.
pub fn should_force_colors() -> bool {
    if std::env::var("FORCE_COLOR").is_ok() {
        return true;
    }
    if matches!(std::env::var("CLICOLOR_FORCE").as_deref(), Ok(v) if v != "0") {
        return true;
    }
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    if matches!(std::env::var("CLICOLOR").as_deref(), Ok("0")) {
        return false;
    }
    // Default to no colors - the backend's stderr is piped (not a TTY).
    false
}
