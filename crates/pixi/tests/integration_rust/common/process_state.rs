//! Serializes access to the process-global state these tests reach for.
//!
//! `cargo test` runs every test in this binary as a thread of a *single*
//! process, so anything a test changes process-wide — an environment variable
//! such as `PIXI_CACHE_DIR`, the working directory — is changed for every
//! other test that happens to be running at that moment, and then disappears
//! again while those tests are still using it. The symptoms are spectacular
//! and land in innocent tests: the temporary cache directory is wiped from
//! under a running solve, or a `git` subprocess inherits a working directory
//! that no longer exists and dies with `fatal: Unable to read current working
//! directory`. See <https://github.com/prefix-dev/pixi/issues/6733>.
//!
//! `cargo nextest` — what CI runs — gives each test its own process and hides
//! the problem completely, which is why this only bites plain `cargo test`.
//!
//! The rule enforced here:
//!
//! * every pixi command the harness runs holds the *shared* side of the lock
//!   ([`guarded`]);
//! * every test that repoints an environment variable ([`with_env_vars`]) or
//!   the working directory ([`with_current_dir`]) holds the *exclusive* side,
//!   for as long as the change is observable.
//!
//! Tests that leave global state alone keep running concurrently with each
//! other; they only wait while one of the few global-state tests is inside its
//! critical section.

use std::{
    cell::Cell,
    ffi::OsStr,
    future::Future,
    hash::Hash,
    path::{Path, PathBuf},
};

use tokio::sync::RwLock;

/// Shared for pixi commands, exclusive for mutations of the environment or the
/// working directory.
static PROCESS_STATE: RwLock<()> = RwLock::const_new(());

/// Which side of [`PROCESS_STATE`] the current thread already holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    Nothing,
    Shared,
    Exclusive,
}

thread_local! {
    /// The section this thread is already inside.
    ///
    /// The test harness runs each test on its own thread and tests await the
    /// commands they run inline, so this reliably describes *this* test's
    /// section. Nesting has to pass straight through rather than lock again:
    /// [`RwLock`] is not reentrant, and it is fair, so even a second shared
    /// guard would queue behind a waiting writer and deadlock.
    static HELD: Cell<Held> = const { Cell::new(Held::Nothing) };
}

/// Records which side of the lock this thread holds, and restores the previous
/// value on drop (including while unwinding).
struct Section(Held);

impl Section {
    fn enter(held: Held) -> Self {
        let previous = HELD.replace(held);
        Self(previous)
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        HELD.set(self.0);
    }
}

/// Runs a pixi command while holding the shared side of the lock, so no test
/// can repoint the environment or the working directory underneath it.
///
/// Every command the harness runs goes through here. Commands nested inside an
/// exclusive section (a test that scoped `PIXI_CACHE_DIR` and then installs)
/// run as-is: that section already excludes everyone else.
pub(crate) async fn guarded<F: Future>(fut: F) -> F::Output {
    if HELD.get() != Held::Nothing {
        return fut.await;
    }
    let _lock = PROCESS_STATE.read().await;
    let _section = Section::enter(Held::Shared);
    fut.await
}

/// Runs `fut` while holding the exclusive side of the lock, locking out every
/// pixi command the harness runs elsewhere in this binary.
async fn exclusive<F: Future>(fut: F) -> F::Output {
    match HELD.get() {
        Held::Exclusive => return fut.await,
        Held::Shared => panic!(
            "cannot change process-global state from inside a pixi command that only holds the \
             shared guard; move the `with_env_vars`/`with_current_dir` call outside it"
        ),
        Held::Nothing => {}
    }
    let _lock = PROCESS_STATE.write().await;
    let _section = Section::enter(Held::Exclusive);
    fut.await
}

/// Applies `vars` to the process environment for the duration of `fut`.
///
/// Use this instead of [`temp_env::async_with_vars`] directly: `temp_env`
/// serializes against other `temp_env` calls, but not against the tests that
/// merely *read* what it changed.
pub(crate) async fn with_env_vars<K, V, F>(vars: impl AsRef<[(K, Option<V>)]>, fut: F) -> F::Output
where
    K: AsRef<OsStr> + Clone + Eq + Hash,
    V: AsRef<OsStr> + Clone,
    F: Future,
{
    exclusive(temp_env::async_with_vars(vars, fut)).await
}

/// Runs `fut` with the process working directory set to `dir`, restoring the
/// previous one afterwards — also when `fut` panics, so a failing test doesn't
/// leave the rest of the binary with a working directory that is about to be
/// deleted.
pub(crate) async fn with_current_dir<F: Future>(dir: &Path, fut: F) -> F::Output {
    exclusive(async move {
        let original = std::env::current_dir().expect("failed to read the working directory");
        let _restore = RestoreCurrentDir(original);
        std::env::set_current_dir(dir)
            .unwrap_or_else(|err| panic!("failed to enter '{}': {err}", dir.display()));
        fut.await
    })
    .await
}

/// Restores the working directory it was built with when dropped.
struct RestoreCurrentDir(PathBuf);

impl Drop for RestoreCurrentDir {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}
