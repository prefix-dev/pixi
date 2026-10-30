use std::hash::{Hash, Hasher};

use rattler_conda_types::{MatchSpec, Subdir};
use xxhash_rust::xxh3::Xxh3;

/// Hash identifying a cached `pixi exec` environment by its specs, channels
/// and platform. The executed command is intentionally not part of the hash.
pub struct EnvironmentHash {
    pub specs: Vec<MatchSpec>,
    pub channels: Vec<String>,
    pub platform: Subdir,
    /// Whether the solve that produced this environment was restricted to
    /// locally available packages. Part of the identity because such a solve
    /// can pick older versions, and the prefix is reused across runs: without
    /// it a restricted environment would be served to later online ones.
    pub offline: bool,
    /// The `exclude-newer` configuration the environment was solved with, as
    /// configured rather than as resolved. A relative cutoff such as `7d`
    /// hashes the same on every run, so the environment is reused: its
    /// packages only get older.
    pub exclude_newer: Option<String>,
}

impl Hash for EnvironmentHash {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Self {
            specs,
            channels,
            platform,
            offline,
            exclude_newer,
        } = self;
        specs.hash(state);
        channels.hash(state);
        platform.hash(state);
        offline.hash(state);
        // Only hashed when set, so the environments solved without a cutoff
        // keep their names.
        if let Some(exclude_newer) = exclude_newer {
            exclude_newer.hash(state);
        }
    }
}

impl EnvironmentHash {
    pub fn new(
        specs: Vec<MatchSpec>,
        channels: Vec<String>,
        platform: Subdir,
        offline: bool,
    ) -> Self {
        let mut specs = specs;
        // Canonical order so spec ordering doesn't change the hash.
        specs.sort_by_cached_key(MatchSpec::to_string);
        Self {
            specs,
            channels,
            platform,
            offline,
            exclude_newer: None,
        }
    }

    /// Sets the `exclude-newer` configuration the environment is solved with,
    /// see [`Self::exclude_newer`].
    pub fn with_exclude_newer(mut self, exclude_newer: Option<String>) -> Self {
        self.exclude_newer = exclude_newer;
        self
    }

    /// Directory name for the cached environment: `{prefix}-{hash}` when a
    /// prefix is given, otherwise just `{hash}`.
    pub fn name(&self, prefix: Option<&str>) -> String {
        let mut hasher = Xxh3::new();
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
    use rattler_conda_types::{MatchSpec, ParseStrictness, Subdir};

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
            Subdir::Linux64,
            false,
        );
        let strip = |s: String| s.rsplit_once('-').unwrap().1.to_string();
        assert_eq!(
            strip(h.name(Some("sh"))),
            strip(h.name(Some("voms-proxy-init"))),
        );
    }

    // The name is a directory on disk, so it must not change between builds.
    #[test]
    fn name_is_stable() {
        let h = EnvironmentHash::new(
            vec![spec("python")],
            vec!["conda-forge".into()],
            Subdir::Linux64,
            false,
        );
        assert_eq!(h.name(Some("python")), "python-5a7a16265bfee075");
    }

    #[test]
    fn name_has_no_prefix_when_caller_passes_none() {
        let h = EnvironmentHash::new(vec![spec("foo")], vec![], Subdir::Linux64, false);
        let name = h.name(None);
        assert!(name.chars().all(|c| c.is_ascii_hexdigit()), "got {name}");
    }

    #[test]
    fn name_depends_on_exclude_newer() {
        let without = EnvironmentHash::new(vec![spec("foo")], vec![], Subdir::Linux64, false);
        let with = |exclude_newer: &str| {
            EnvironmentHash::new(vec![spec("foo")], vec![], Subdir::Linux64, false)
                .with_exclude_newer(Some(exclude_newer.to_string()))
        };
        assert_ne!(without.name(None), with("7d").name(None));
        assert_ne!(with("7d").name(None), with("14d").name(None));
        assert_eq!(with("7d").name(None), with("7d").name(None));
    }

    #[test]
    fn name_uses_caller_provided_prefix() {
        let h = EnvironmentHash::new(
            vec![spec("extra"), spec("cmd")],
            vec![],
            Subdir::Linux64,
            false,
        );
        assert!(h.name(Some("cmd")).starts_with("cmd-"));
    }

    #[test]
    fn name_ignores_spec_order() {
        let a = EnvironmentHash::new(
            vec![spec("foo"), spec("bar")],
            vec![],
            Subdir::Linux64,
            false,
        );
        let b = EnvironmentHash::new(
            vec![spec("bar"), spec("foo")],
            vec![],
            Subdir::Linux64,
            false,
        );
        assert_eq!(a.name(None), b.name(None));
    }

    #[test]
    fn name_changes_when_specs_change() {
        let a = EnvironmentHash::new(vec![spec("foo")], vec![], Subdir::Linux64, false);
        let b = EnvironmentHash::new(vec![spec("bar")], vec![], Subdir::Linux64, false);
        assert_ne!(a.name(None), b.name(None));
    }

    #[test]
    fn name_changes_when_platform_changes() {
        let a = EnvironmentHash::new(vec![spec("foo")], vec![], Subdir::Linux64, false);
        let b = EnvironmentHash::new(vec![spec("foo")], vec![], Subdir::Osx64, false);
        assert_ne!(a.name(None), b.name(None));
    }

    #[test]
    fn name_changes_when_channel_order_changes() {
        let a = EnvironmentHash::new(
            vec![spec("foo")],
            vec!["conda-forge".into(), "bioconda".into()],
            Subdir::Linux64,
            false,
        );
        let b = EnvironmentHash::new(
            vec![spec("foo")],
            vec!["bioconda".into(), "conda-forge".into()],
            Subdir::Linux64,
            false,
        );
        assert_ne!(a.name(None), b.name(None));
    }

    // Regression test for https://github.com/prefix-dev/pixi/issues/7165.
    #[test]
    fn name_changes_when_zero_component_moves() {
        for (a, b) in [
            ("maturin==1.0.1", "maturin==1.1.0"),
            ("maturin==13.0.1", "maturin==13.1.0"),
        ] {
            let ha = EnvironmentHash::new(vec![spec(a)], vec![], Subdir::Linux64, false);
            let hb = EnvironmentHash::new(vec![spec(b)], vec![], Subdir::Linux64, false);
            assert_ne!(ha.name(None), hb.name(None), "{a} vs {b}");
        }
    }
}
