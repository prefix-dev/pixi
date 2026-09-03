use std::{env, process::Command};

/// Records the commit `pixi` is built from in the `PIXI_GIT_SHA` compile-time
/// environment variable, read back by `main.rs`. It lives in the binary crate
/// so that moving to a new commit only invalidates this package.
///
/// Set `PIXI_GIT_SHA` in the build environment to supply it directly, e.g. when
/// building from a source archive that carries no git metadata. With neither
/// available `pixi info` omits the `Git SHA` line and reports `null` in its
/// JSON output.
fn main() {
    println!("cargo::rerun-if-env-changed=PIXI_GIT_SHA");
    if env::var_os("PIXI_GIT_SHA").is_some() {
        return;
    }

    let Some(sha) = git(&["rev-parse", "--short=9", "HEAD"]) else {
        return;
    };
    println!("cargo::rustc-env=PIXI_GIT_SHA={sha}");

    // Rebuild when HEAD moves so the recorded SHA cannot go stale.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo::rerun-if-changed={git_dir}/HEAD");
        if let Some(head_ref) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
            println!("cargo::rerun-if-changed={git_dir}/{head_ref}");
        }
    }
}

/// Runs `git` with `args`, returning its trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let stdout = stdout.trim();
    (!stdout.is_empty()).then(|| stdout.to_string())
}
