//! Compatibility test catalog for the full pixi configuration schema.
//!
//! The pixi-side mirror of `rattler_config`'s `tests/compat.rs`: where that
//! catalog pins the contract of the *shared* keys, this one pins the whole
//! pixi schema — the shared keys (through `ConfigBase`/`CommonConfig`) plus
//! pixi's `PixiConfig` extension keys — so upgrading the shared config crate
//! can never silently break a user's existing `config.toml`:
//!
//! 1. **Parsing permutations** — canonical kebab-case, legacy `snake_case`
//!    aliases, a realistic old pixi config with deprecated keys, and typos
//!    (fixtures in `test-data/compat/`). Deprecated/unknown keys must parse
//!    with warnings, never hard errors.
//! 2. **Round-trip stability** — load → serialize → load is lossless and
//!    serialization is idempotent, so `pixi config set` + save never
//!    corrupts the rest of a user's configuration.
//! 3. **Editing matrix** — every settable key family goes through the public
//!    `Config::set`; set+unset on a pristine config restores the default,
//!    proving edits have no collateral effect on unrelated keys.
//! 4. **Merge semantics** — how two layered files combine (replace vs extend
//!    vs field-wise merge), including the extension fields' legacy quirks.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use pixi_config::{Config, DetachedEnvironments, PinningStrategy, TlsRootCerts};
use rattler_conda_types::{ChannelConfig, Platform};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-data/compat")
        .join(name)
}

fn parse(name: &str) -> (Config, BTreeSet<String>) {
    let toml = fs_err::read_to_string(fixture_path(name)).unwrap();
    Config::from_toml(&toml, Some(Path::new(name)))
        .unwrap_or_else(|e| panic!("{name} must parse: {e}"))
}

/// Strip machine-dependent values before snapshotting: `channel_config`
/// is serde-skipped and its default embeds the current working directory,
/// and `concurrency.solves` defaults to the local CPU count. `solves` is
/// normalized *unconditionally* (a conditional "only when it equals the
/// default" would flip whenever an explicitly configured value happens to
/// match the CPU count); explicitly configured values are asserted in
/// `explicit_concurrency_values_parse` instead.
fn normalized(mut config: Config) -> Config {
    config.channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("/root/dir"));
    config.concurrency.solves = 0;
    config
}

/// Explicitly configured concurrency values parse correctly. Kept out of
/// the snapshots because `normalized` blanks `solves` (see there).
#[test]
fn explicit_concurrency_values_parse() {
    let (config, _) = parse("kitchen-sink.toml");
    assert_eq!(config.concurrency.solves, 4);

    let (config, _) = parse("legacy-config.toml");
    assert_eq!(config.concurrency.solves, 2);

    let (config, _) = parse("override-layer.toml");
    assert_eq!(config.concurrency.solves, 9);
}

const FIXTURES: &[&str] = &[
    "kitchen-sink.toml",
    "snake-case-aliases.toml",
    "legacy-config.toml",
    "typos.toml",
    "override-layer.toml",
];

/// Every fixture must parse; snapshot the parsed result and the reported
/// unused keys so schema changes are reviewed consciously.
#[test]
fn parsing_permutations() {
    for name in FIXTURES {
        let (config, unused) = parse(name);
        insta::assert_debug_snapshot!(format!("parse__{name}"), (unused, normalized(config)));
    }
}

/// Snake-case spellings must parse to the same configuration as their
/// kebab-case equivalents, without any unused-key warnings.
#[test]
fn snake_case_aliases_are_equivalent() {
    let (config, unused) = parse("snake-case-aliases.toml");
    assert!(unused.is_empty(), "aliases must not warn: {unused:?}");

    let canonical = r#"
        default-channels = ["conda-forge"]
        authentication-override-file = "/path/to/auth.json"
        tls-no-verify = true
        tls-root-certs = "system"
        allow-symbolic-links = false
        allow-hard-links = true
        allow-ref-links = false
        change-ps1 = false

        [repodata-config]
        disable-bzip2 = true
        disable-zstd = true
    "#;
    let (canonical, _) = Config::from_toml(canonical, None).unwrap();
    assert_eq!(config, canonical);
}

/// A realistic old pixi config: deprecated keys and legacy value spellings
/// must keep parsing, and the deprecated values must still take effect
/// through the modern accessors.
#[test]
fn legacy_config_still_takes_effect() {
    let (config, unused) = parse("legacy-config.toml");

    // The only unknown key is the long-removed repodata-config.disable-jlap;
    // the deprecated pixi keys are still consumed by the schema itself.
    assert_eq!(
        unused.into_iter().collect::<Vec<_>>(),
        ["repodata-config.disable-jlap"]
    );

    // Deprecated top-level keys are folded into their `shell.*` replacements
    // at load time, so the modern accessors see them.
    assert!(!config.change_ps1(), "change-ps1 = false must take effect");
    assert!(
        config.force_activate(),
        "force-activate = true must take effect"
    );
    assert_eq!(config.shell().change_ps1, Some(false));
    assert_eq!(config.shell().force_activate, Some(true));

    // The legacy `"native"` spelling resolves to the system trust store.
    assert_eq!(config.tls_root_certs(), Some(TlsRootCerts::System));

    // The boolean variant of the bool-or-path key.
    assert_eq!(
        config.detached_environments(),
        DetachedEnvironments::Boolean(true)
    );

    // Ordinary keys next to the deprecated ones survive.
    assert_eq!(config.pinning_strategy(), Some(PinningStrategy::Minor));
    assert_eq!(config.max_concurrent_solves(), 2);
    assert_eq!(config.repodata_config.default.disable_bzip2, Some(true));
    assert_eq!(
        config
            .pypi_config()
            .index_url
            .as_ref()
            .map(url::Url::as_str),
        Some("https://pypi.org/simple")
    );
}

/// Misspelled keys — shared and extension, top level and nested — are all
/// reported as unused (pixi warns), while their valid neighbors still load.
#[test]
fn typos_warn_but_neighbors_survive() {
    let (config, unused) = parse("typos.toml");

    for key in [
        "pinning-strateggy",              // extension, top level
        "authentification-override-file", // shared, top level
        "shell.chnge-ps1",                // extension, nested
        "concurrency.solvs",              // shared, nested
        "pypi-config.index-urll",         // extension, nested
    ] {
        assert!(
            unused.contains(key),
            "{key} must be reported, got {unused:?}"
        );
    }

    assert_eq!(config.default_channels.as_ref().map(Vec::len), Some(1));
    assert!(config.force_activate());
    assert_eq!(config.max_concurrent_downloads(), 8);
}

/// load → serialize → load must be lossless, and serialization idempotent.
/// This is what guarantees that `pixi config set` + `save` never corrupts
/// the rest of a user's configuration. Note this holds even for the legacy
/// fixture: the deprecated keys re-serialize and re-fold identically.
#[test]
fn round_trip_is_lossless_and_idempotent() {
    for name in FIXTURES {
        let (config, _) = parse(name);
        let first = config.to_toml().unwrap();
        let (reloaded, unused) = Config::from_toml(&first, None).unwrap();
        assert!(
            unused.is_empty(),
            "{name}: serialization must not invent unknown keys: {unused:?}"
        );
        assert_eq!(config, reloaded, "{name}: round trip must be lossless");
        let second = reloaded.to_toml().unwrap();
        assert_eq!(first, second, "{name}: serialization must be idempotent");
    }
}

/// The editing matrix: one entry per settable key family with a
/// representative value, all driven through the public `Config::set` (the
/// same code path as `pixi config set`). Extend this list whenever a key is
/// added to `CommonConfig` or `PixiConfig`.
const EDIT_MATRIX: &[(&str, &str)] = &[
    // ---- shared keys ----
    ("default-channels", r#"["conda-forge"]"#),
    ("authentication-override-file", "/tmp/auth.json"),
    ("tls-no-verify", "true"),
    ("tls-root-certs", "webpki"),
    (
        "mirrors",
        r#"{"https://conda.anaconda.org/conda-forge": ["https://prefix.dev/conda-forge"]}"#,
    ),
    ("run-post-link-scripts", "insecure"),
    ("allow-symbolic-links", "false"),
    ("allow-hard-links", "true"),
    ("allow-ref-links", "false"),
    ("build.package-format", "conda:max"),
    ("repodata-config.disable-bzip2", "true"),
    ("repodata-config.disable-zstd", "false"),
    ("repodata-config.disable-sharded", "true"),
    ("concurrency.solves", "3"),
    ("concurrency.downloads", "21"),
    ("proxy-config.https", "https://proxy.example.com:8080"),
    ("proxy-config.http", "http://proxy.example.com:8080"),
    ("proxy-config.non-proxy-hosts", r#"["localhost"]"#),
    (
        "s3-options.some-bucket",
        r#"{"endpoint-url": "https://s3.example.com", "region": "auto", "force-path-style": true}"#,
    ),
    // ---- pixi extension keys ----
    ("pinning-strategy", "no-pin"),
    ("detached-environments", "true"),
    ("detached-environments", "false"),
    ("detached-environments", "/opt/pixi/envs"),
    ("tool-platform", "linux-64"),
    ("pypi-config.index-url", "https://pypi.org/simple"),
    (
        "pypi-config.extra-index-urls",
        r#"["https://example.com/simple"]"#,
    ),
    ("pypi-config.keyring-provider", "subprocess"),
    ("pypi-config.allow-insecure-host", r#"["localhost:8080"]"#),
    ("shell.change-ps1", "false"),
    ("shell.force-activate", "true"),
    ("shell.source-completion-scripts", "false"),
    ("experimental.use-environment-activation-cache", "true"),
    // `[cache]` paths must be absolute (Config::set re-validates), so the
    // matrix uses absolute values.
    ("cache.root", "/shared/pixi/cache"),
    ("cache.conda-packages", "/shared/pixi/cache/pkgs"),
    ("cache.repodata", "/scratch/pixi/repodata"),
    ("cache.pypi-wheels", "/scratch/pixi/wheels"),
    ("cache.pypi-mapping", "/scratch/pixi/mapping"),
    ("cache.exec-environments", "/scratch/pixi/exec"),
    ("cache.build-tool-environments", "/scratch/pixi/build-tools"),
    ("cache.detached-environments", "/shared/pixi/envs"),
    ("cache.netfs-redirect", "never"),
];

/// Every key in the matrix can be set on a fully populated config, the
/// result still round-trips, and set+unset on a pristine config restores
/// the default (proving no collateral damage to unrelated keys).
#[test]
fn edit_matrix_set_roundtrip_unset() {
    let (kitchen_sink, _) = parse("kitchen-sink.toml");

    for (key, value) in EDIT_MATRIX {
        // set on a fully populated config …
        let mut edited = kitchen_sink.clone();
        edited
            .set(key, Some((*value).to_string()))
            .unwrap_or_else(|e| panic!("set {key}={value} must succeed: {e}"));

        // … and the result still round-trips losslessly.
        let toml = edited.to_toml().unwrap();
        let (reloaded, unused) = Config::from_toml(&toml, None).unwrap();
        assert!(unused.is_empty(), "{key}: no unknown keys after edit");
        assert_eq!(edited, reloaded, "{key}: round trip after edit");

        // set + unset on a pristine config restores the default state,
        // proving the edit touched nothing else.
        let mut pristine = Config::default();
        pristine.set(key, Some((*value).to_string())).unwrap();
        assert_ne!(pristine, Config::default(), "{key}: set must change state");
        pristine.set(key, None).unwrap();
        assert_eq!(
            pristine,
            Config::default(),
            "{key}: unset must restore the default without collateral changes"
        );
    }
}

/// The deprecated top-level keys are a hard error in `set` (unlike file
/// loading, where they are tolerated and folded): interactive edits must
/// steer users to the `shell.*` replacements.
#[test]
fn edit_rejects_deprecated_keys_with_pointer() {
    let mut config = Config::default();
    for (old, new) in [
        ("change-ps1", "shell.change-ps1"),
        ("force-activate", "shell.force-activate"),
    ] {
        let err = config
            .set(old, Some("true".to_string()))
            .expect_err("deprecated keys must hard-error in set");
        assert!(
            err.to_string().contains(new),
            "error for {old} must point at {new}: {err}"
        );
    }
}

/// Unknown keys must be rejected by `set` (both set and unset direction),
/// for shared and extension key typos alike.
#[test]
fn edit_rejects_unknown_keys() {
    let mut config = Config::default();
    for key in [
        "definitely-a-typo",
        "pinning-strateggy",
        "shell.chnge-ps1",
        "concurrency.bogus",
    ] {
        assert!(
            config.set(key, Some("1".to_string())).is_err(),
            "set {key} must be rejected"
        );
        assert!(
            config.set(key, None).is_err(),
            "unset {key} must be rejected"
        );
    }
}

/// Merge semantics per key family: scalars are replaced, mirrors/s3 extend,
/// nested tables merge field-wise, and the pixi extension fields keep their
/// legacy semantics. Snapshot the merged result so semantic changes are
/// reviewed consciously.
#[test]
fn merge_semantics() {
    let (base, _) = parse("kitchen-sink.toml");
    let (layer, _) = parse("override-layer.toml");
    // `other` (the layer) has the higher priority.
    let merged = base.merge_config(layer);

    // ---- shared keys ----
    // scalars/lists: later layer replaces.
    assert_eq!(
        merged.default_channels.as_ref().map(|c| c[0].to_string()),
        Some("robostack".to_string())
    );
    assert_eq!(merged.tls_no_verify, Some(true));
    // maps: later layer extends.
    assert_eq!(merged.mirrors.len(), 2);
    assert_eq!(merged.s3_options.0.len(), 2);
    // nested tables merge field-wise; unset fields keep the lower layer.
    assert_eq!(merged.repodata_config.default.disable_bzip2, Some(false));
    assert_eq!(merged.repodata_config.default.disable_zstd, Some(false));
    let prefix_dev = url::Url::parse("https://prefix.dev").unwrap();
    let per_channel = &merged.repodata_config.per_channel[&prefix_dev];
    assert_eq!(per_channel.disable_sharded, Some(true)); // from the base layer
    assert_eq!(per_channel.disable_zstd, Some(true)); // from the override layer
    // concurrency: explicitly set values win over the lower layer.
    assert_eq!(merged.max_concurrent_solves(), 9);
    assert_eq!(merged.max_concurrent_downloads(), 12);

    // ---- pixi extension keys ----
    // scalars: later layer replaces.
    assert_eq!(merged.pinning_strategy(), Some(PinningStrategy::Semver));
    assert_eq!(
        merged.detached_environments(),
        DetachedEnvironments::Boolean(false)
    );
    // QUIRK (pinned on purpose): for `tool-platform` the LOWER layer wins.
    // This is the long-standing behavior of the former `Config::merge_config`
    // (`self.tool_platform.or(other.tool_platform)`), preserved verbatim in
    // `PixiConfig::merge_config`. If this assertion ever fails, the merge
    // direction was changed — make that a conscious, documented decision.
    assert_eq!(merged.tool_platform(), Platform::Linux64);
    // pypi-config: index-url replaces, the url/host lists EXTEND (base
    // first), keyring-provider keeps the base when the layer is silent.
    let pypi = merged.pypi_config();
    assert_eq!(
        pypi.index_url.as_ref().map(url::Url::as_str),
        Some("https://test.pypi.org/simple")
    );
    assert_eq!(
        pypi.extra_index_urls
            .iter()
            .map(url::Url::as_str)
            .collect::<Vec<_>>(),
        [
            "https://example.com/simple",
            "https://extra.example.com/simple"
        ]
    );
    assert_eq!(pypi.allow_insecure_host, vec!["localhost:8080".to_string()]);
    assert!(matches!(
        pypi.keyring_provider,
        Some(pixi_config::KeyringProvider::Subprocess)
    ));
    // shell: field-wise merge.
    assert_eq!(merged.shell().change_ps1, Some(true)); // overridden
    assert_eq!(merged.shell().force_activate, Some(true)); // from the base
    assert!(!merged.shell().source_completion_scripts()); // from the base
    // experimental: later layer replaces when set.
    assert!(!merged.experimental_activation_cache_usage());
    // cache: field-wise merge; non-default netfs-redirect wins.
    assert_eq!(
        merged.cache().root.as_deref(),
        Some(Path::new("/other/pixi/cache"))
    );
    assert_eq!(
        merged.cache().conda_packages.as_deref(),
        Some(Path::new("/shared/pixi/cache/pkgs"))
    );
    assert_eq!(
        merged.cache().netfs_redirect,
        pixi_config::NetfsRedirect::Always
    );

    insta::assert_snapshot!(
        "merge__kitchen_sink_plus_override",
        merged.to_toml().unwrap()
    );
}
