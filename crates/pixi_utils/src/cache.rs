use std::hash::{DefaultHasher, Hash, Hasher};

use rattler_conda_types::{MatchSpec, Platform};

/// Hash identifying a cached `pixi exec` environment by its specs, channels
/// and platform. The executed command is intentionally not part of the hash.
#[derive(Hash)]
pub struct EnvironmentHash {
    pub specs: Vec<MatchSpec>,
    pub channels: Vec<String>,
    pub platform: Platform,
}

impl EnvironmentHash {
    pub fn new(specs: Vec<MatchSpec>, channels: Vec<String>, platform: Platform) -> Self {
        let mut specs = specs;
        // Canonical order so spec ordering doesn't change the hash.
        specs.sort_by_cached_key(MatchSpec::to_string);
        Self {
            specs,
            channels,
            platform,
        }
    }

    /// Directory name for the cached environment: `{prefix}-{hash}` when a
    /// prefix is given, otherwise just `{hash}`.
    pub fn name(&self, prefix: Option<&str>) -> String {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        let hash = hasher.finish();
        match prefix {
            Some(prefix) => format!("{prefix}-{hash:x}"),
            None => format!("{hash:x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{MatchSpec, ParseStrictness, Platform};

    use super::EnvironmentHash;

    fn spec(s: &str) -> MatchSpec {
        MatchSpec::from_str(s, ParseStrictness::Lenient).unwrap()
    }

    // Regression test for https://github.com/prefix-dev/pixi/discussions/6034.
    #[test]
    fn name_does_not_depend_on_command() {
        let h = EnvironmentHash::new(
            vec![spec("rucio-mcp")],
            vec!["conda-forge".into()],
            Platform::Linux64,
        );
        let strip = |s: String| s.rsplit_once('-').unwrap().1.to_string();
        assert_eq!(
            strip(h.name(Some("sh"))),
            strip(h.name(Some("voms-proxy-init"))),
        );
    }

    #[test]
    fn name_has_no_prefix_when_caller_passes_none() {
        let h = EnvironmentHash::new(vec![spec("foo")], vec![], Platform::Linux64);
        let name = h.name(None);
        assert!(name.chars().all(|c| c.is_ascii_hexdigit()), "got {name}");
    }

    #[test]
    fn name_uses_caller_provided_prefix() {
        let h = EnvironmentHash::new(vec![spec("extra"), spec("cmd")], vec![], Platform::Linux64);
        assert!(h.name(Some("cmd")).starts_with("cmd-"));
    }

    #[test]
    fn name_ignores_spec_order() {
        let a = EnvironmentHash::new(vec![spec("foo"), spec("bar")], vec![], Platform::Linux64);
        let b = EnvironmentHash::new(vec![spec("bar"), spec("foo")], vec![], Platform::Linux64);
        assert_eq!(a.name(None), b.name(None));
    }

    #[test]
    fn name_changes_when_specs_change() {
        let a = EnvironmentHash::new(vec![spec("foo")], vec![], Platform::Linux64);
        let b = EnvironmentHash::new(vec![spec("bar")], vec![], Platform::Linux64);
        assert_ne!(a.name(None), b.name(None));
    }

    #[test]
    fn name_changes_when_platform_changes() {
        let a = EnvironmentHash::new(vec![spec("foo")], vec![], Platform::Linux64);
        let b = EnvironmentHash::new(vec![spec("foo")], vec![], Platform::Osx64);
        assert_ne!(a.name(None), b.name(None));
    }

    #[test]
    fn name_changes_when_channel_order_changes() {
        let a = EnvironmentHash::new(
            vec![spec("foo")],
            vec!["conda-forge".into(), "bioconda".into()],
            Platform::Linux64,
        );
        let b = EnvironmentHash::new(
            vec![spec("foo")],
            vec!["bioconda".into(), "conda-forge".into()],
            Platform::Linux64,
        );
        assert_ne!(a.name(None), b.name(None));
    }
}
