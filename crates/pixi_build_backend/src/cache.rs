use std::collections::HashMap;

use itertools::Itertools;
use pixi_build_types::SourcePackageName;
use rattler_conda_types::PackageName;

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
