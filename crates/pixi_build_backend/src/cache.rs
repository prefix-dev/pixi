use std::collections::HashMap;

use itertools::Itertools;

use crate::{ProjectModel, Targets, traits::Dependencies};

pub fn sccache_tools() -> Vec<String> {
    vec!["sccache".to_string()]
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

pub fn add_sccache<'a, P: ProjectModel>(
    dependencies: &mut Dependencies<'a, <P::Targets as Targets>::Spec>,
    sccache_tools: &'a [String],
    empty_spec: &'a <<P as ProjectModel>::Targets as Targets>::Spec,
) {
    for cache_tool in sccache_tools {
        if !dependencies.build.contains_key(&cache_tool) {
            dependencies.build.insert(cache_tool, empty_spec);
        }
    }
}
