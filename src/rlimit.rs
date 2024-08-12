/// The desired value for the RLIMIT_NOFILE resource limit. This is the number
/// of file descriptors that pixi should be able to open.
pub const DESIRED_RLIMIT_NOFILE: u64 = 1024;

/// Attempt to increase the RLIMIT_NOFILE resource limit to the desired value
/// for pixi. The desired value is defined by the `DESIRED_RLIMIT_NOFILE`
/// constant and should suffice for most use cases.
#[cfg(not(win))]
pub(crate) fn try_increase_rlimit_to_sensible() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(
        || match rlimit::increase_nofile_limit(DESIRED_RLIMIT_NOFILE) {
            Ok(DESIRED_RLIMIT_NOFILE) => {
                tracing::debug!("Increased RLIMIT_NOFILE to {}", DESIRED_RLIMIT_NOFILE);
            }
            Ok(lim) => {
                if lim < DESIRED_RLIMIT_NOFILE {
                    tracing::info!(
                        "Attempted to set RLIMIT_NOFILE to {} but was only able to set it to {}",
                        DESIRED_RLIMIT_NOFILE,
                        lim
                    );
                } else {
                    tracing::debug!(
                        "Attempted to set RLIMIT_NOFILE to {} but was already set to {}",
                        DESIRED_RLIMIT_NOFILE,
                        lim
                    );
                }
            }
            Err(err) => {
                tracing::info!(
                    "Attempted to set RLIMIT_NOFILE to {} failed: {err}",
                    DESIRED_RLIMIT_NOFILE
                );
            }
        },
    );
}

#[cfg(win)]
pub fn increase_rlimit_to_desired() {
    // On Windows, there is no need to increase the RLIMIT_NOFILE resource
    // limit.
}
