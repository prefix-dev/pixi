use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Determines the default author based on the default git author. Both the name and the email
/// address of the author are returned.
pub fn get_default_author() -> Option<(String, String)> {
    let rv = Command::new("git")
        .arg("config")
        .arg("--get-regexp")
        .arg("^user.(name|email)$")
        .stdout(Stdio::piped())
        .output()
        .ok()?;

    let mut name = None;
    let mut email = None;

    for line in std::str::from_utf8(&rv.stdout).ok()?.lines() {
        match line.split_once(' ') {
            Some(("user.email", value)) => {
                email = Some(value.to_string());
            }
            Some(("user.name", value)) => {
                name = Some(value.to_string());
            }
            _ => {}
        }
    }

    Some((name?, email.unwrap_or_else(|| "".into())))
}

/// Returns the default cache directory.
/// Most important is the `PIXI_CACHE_DIR` environment variable.
/// If that is not set, the `RATTLER_CACHE_DIR` environment variable is used.
/// If that is not set, the default cache directory of [`rattler::default_cache_dir`] is used.
pub fn get_cache_dir() -> miette::Result<PathBuf> {
    std::env::var("PIXI_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("RATTLER_CACHE_DIR").map(PathBuf::from))
        .or_else(|_| {
            rattler::default_cache_dir()
                .map_err(|_| miette::miette!("could not determine default cache directory"))
        })
}
