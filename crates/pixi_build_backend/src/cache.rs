use std::collections::HashMap;

use itertools::Itertools;
use pixi_build_types::SourcePackageName;
use rattler_conda_types::PackageName;

use crate::{ProjectModel, Targets, traits::Dependencies};

pub fn sccache_tools() -> Vec<SourcePackageName> {
    vec![SourcePackageName::from(PackageName::new_unchecked(
        "sccache",
    ))]
}

/// Return environment variables that are used by sccache.
pub fn sccache_envs(env: &HashMap<String, String>) -> Option<Vec<&str>> {
    let res = env
        .keys()
        .filter(|k| k.starts_with("SCCACHE"))
        .map(|k| k.as_str())
        .collect_vec();
    if res.is_empty() { None } else { Some(res) }
}

/// Ensure the binaries for a globally-configured compiler cache are available
/// on `PATH`.
///
/// A compiler cache that comes from the user's global pixi config is a
/// per-machine preference, so it is used as a compiler launcher only and is
/// deliberately *not* added to the build requirements (doing so would make the
/// lockfile depend on who runs the resolve). That means the tool has to be
/// installed on the machine already; if it is missing we fail with an
/// actionable hint instead of silently building without the cache.
pub fn ensure_compiler_cache_on_path(tools: &[SourcePackageName]) -> miette::Result<()> {
    for tool in tools {
        let name = tool.as_str();
        if which::which(name).is_err() {
            return Err(miette::miette!(
                help = format!("install it with `pixi global install {name}`"),
                "the global `compiler-cache` config requests `{name}`, but it was not found on PATH",
            ));
        }
    }
    Ok(())
}

pub fn add_sccache<'a, P: ProjectModel>(
    dependencies: &mut Dependencies<'a, <P::Targets as Targets>::Spec>,
    sccache_tools: &'a [SourcePackageName],
    empty_spec: &'a <<P as ProjectModel>::Targets as Targets>::Spec,
) {
    for cache_tool in sccache_tools {
        if !dependencies.build.contains_key(cache_tool) {
            dependencies.build.insert(cache_tool, empty_spec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_compiler_cache_on_path_errors_when_missing() {
        let tool = SourcePackageName::from(PackageName::new_unchecked(
            "pixi-definitely-not-installed-cache",
        ));
        let err = ensure_compiler_cache_on_path(std::slice::from_ref(&tool))
            .expect_err("a non-existent tool must not be found on PATH");

        // The hint should point users at `pixi global install`. Collapse
        // whitespace first since miette wraps long lines in its rendering.
        let rendered = format!("{err:?}");
        let collapsed = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            collapsed.contains("pixi global install"),
            "error should suggest `pixi global install`, got: {rendered}"
        );
    }
}
