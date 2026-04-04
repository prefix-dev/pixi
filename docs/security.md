# Supply Chain Security

Supply chain security starts with a simple assumption: even when your own code is correct, the packages you install can still become the attack path.

Typical risks include:

- compromised or tampered artifacts;
- hostile or hijacked upstream releases;
- newly published packages that later turn out to be malicious or broken;
- known-vulnerable dependencies that stay in your environment because nothing forces an update;
- poor visibility into what actually changed between one environment revision and the next.

Pixi does not eliminate these risks, and it is not a vulnerability scanner for conda environments. What it does provide is a set of practical controls that help you reduce exposure, review changes more clearly, and respond faster when something goes wrong.

This is the security model we recommend, step by step:

1. make dependency resolution reproducible and reviewable;
2. delay very fresh uploads when a cooldown is appropriate;
3. respond to advisories by constraining or overriding dependencies;
4. scan the installed environment with tooling that understands what is actually on disk;
5. add attestations when you publish your own artifacts.

## 1. Make Dependency Resolution Reproducible

**What it is**

Pixi records the fully resolved environment in `pixi.lock`, including the exact artifacts that were selected.

**What it helps against**

This reduces the risk of silent dependency drift and makes unexpected package changes visible in code review. Instead of installing "whatever is newest right now", your environment is tied to explicit artifacts.

**How it works**

Once an environment is resolved, future installs use the locked artifacts from `pixi.lock`. That gives you a stable review surface: if a dependency changes, the lock file changes too.

To review lock file changes between commits in a human-readable way, you can use [`pixi-diff`](integration/extensions/pixi_diff.md) directly or integrate the output into CI with [`pixi-diff-to-markdown`](integration/ci/updates_github_actions.md).

For example:

```bash
# bash / zsh
pixi diff --before <(git show HEAD~20:pixi.lock) --after pixi.lock | pixi diff-to-markdown

# fish
pixi diff --before (git show HEAD~20:pixi.lock | psub) --after pixi.lock | pixi diff-to-markdown
```

**How to implement it**

Keep `pixi.lock` under version control, review lock file diffs in pull requests, and treat unexpected artifact changes as a security-relevant event.

## 2. Delay Fresh Uploads With `exclude-newer`

**What it is**

[`exclude-newer`](reference/pixi_manifest.md#exclude-newer-optional) lets you ignore packages uploaded too recently.

**What it helps against**

This is a practical defense against compromised releases, rushed hostile uploads, or ecosystem incidents where a bad package is published and only detected shortly afterwards.

The delay is useful even when public ecosystems already run security checks. In practice, those checks are often asynchronous. For example, package indexes may publish first and only then run third-party analysis that later flags a release. A cooldown of a few days gives those external checks, user reports, and ecosystem response processes time to surface problems before your resolver consumes the new artifact.

**How it works**

Pixi can apply a default delay across your workspace and then let you opt specific channels or packages back in immediately when needed. This is useful when you want a conservative trust window for public ecosystems, while still allowing trusted internal channels or urgent fixes through without delay.

```toml
[workspace]
name = "my-workspace"
channels = [
  "conda-forge",
  # get most recent versions of packages you control
  { channel = "https://prefix.dev/my-internal-channel", exclude-newer = "0d" },
]
exclude-newer = "14d"
platforms = ["linux-64", "osx-arm64", "win-64"]

[dependencies]
# CVE-XXXX-YYYY: allow the fresh fixed build immediately
python = { version = "3.12.*", exclude-newer = "0d" }

[constraints]
# CVE-XXXX-YYYY: allow the fresh fixed build immediately
openssl = { exclude-newer = "0d" }
```

In that example:

- all packages are delayed by 14 days by default;
- packages from the internal channel are not delayed;
- `openssl` and `python` are allowed through immediately, even if the fixed build is fresh.

**How to implement it**

Start with a default delay for public channels, then carve out exceptions only for artifacts you control or for urgent security fixes that you have explicitly reviewed.

!!! tip "CEP for `upload_timestamp` in repodata"
    There is also an in-progress proposal, [conda/ceps#154](https://github.com/conda/ceps/pull/154), to include upload timestamps in `repodata.json`. If adopted, that would let tools consume channel-provided upload times directly and harden this workflow against spoofed timestamp entries from conda-forge itself.
    Once this CEP is implemented, Pixi will switch the behavior of `exclude-newer` to use the `upload_timestamp` instead of the `timestamp`.

## 3. Respond To Advisories With Constraints And Overrides

**What it is**

When a CVE affects one of your dependencies, the preferred response is to move to a non-vulnerable version. In some cases, that also means temporarily relaxing `exclude-newer` for the fixed package so you can adopt the security release immediately.

If another dependency prevents the solver from reaching the fixed version, Pixi supports [`dependency-overrides`](advanced/override.md) for PyPI packages.

**What it helps against**

This helps you react when a vulnerable version is already known, especially when transitive dependency bounds would otherwise keep you stuck on an affected release.

**How it works**

Overrides let you force dependency requirements during resolution so the solver can select a safe version even if upstream packages have not caught up yet.

!!! note "PyPI-only for now"
    We are planning to support a similar feature like this for Conda packages as well. For more information, see [pixi#4891](https://github.com/prefix-dev/pixi/issues/4891).

```toml
[pypi-options.dependency-overrides]
# force all packages to depend on urllib3 >=2.2.2
urllib3 = ">=2.2.2"
```

These controls are complementary to `exclude-newer`: `exclude-newer` reduces exposure to newly uploaded artifacts, while constraints and overrides help you respond once a vulnerable version is already known.

**How to implement it**

When an advisory lands, update to the fixed version first. If the solver is blocked by transitive bounds, add the narrowest override that gets you onto the non-vulnerable release and remove it again once upstream metadata catches up.

## 4. Scan The Installed Environment Directly

**What it is**

For CVE analysis today, our preferred workflow is to scan the installed Pixi-managed environment directly with [Syft](https://github.com/anchore/syft) and a vulnerability scanner such as Grype.

Generating an SBOM is still useful when you want to archive the inventory or share it with someone else, but it does not need to be the default intermediate step for local or CI vulnerability analysis.

**What it helps against**

This improves visibility. It helps you understand what is actually present on disk, which is what your security tooling ultimately needs to analyze.

It is especially relevant in the conda ecosystem because cross-ecosystem vulnerability matching is still improving. Today, an installed-environment scan is often the most practical way to bridge that gap.

**How it works**

Instead of scanning only what was requested in `pixi.toml`, you scan the concrete environment directory. Internally, we recommend enabling the conda and auditable Rust catalogers explicitly so the behavior stays consistent across different scan targets:

```bash
syft .pixi/envs/default \
  --select-catalogers=+conda-meta-cataloger,+cargo-auditable-binary-cataloger \
  --output syft-json=syft-output.json
```

That output can then be fed to Grype for CVE scanning:

```bash
grype syft-output.json
```

This matters because Syft will detect conda packages from `conda-meta` when scanning a filesystem location like `.pixi/envs/default`, but it does not always do so by default when scanning a container image that contains a conda environment. Passing the catalogers explicitly avoids that surprise.

If you want to continue straight into vulnerability analysis, prefer feeding Syft's own output into your scanner instead of converting through CycloneDX first. In practice, format conversion can lose information, and scanning a CycloneDX export can produce different results from scanning Syft's native output directly.

For conda packages specifically, Syft currently tends to emit CPEs but not PURLs. That means Grype may need to be configured to match on CPEs if you want useful conda vulnerability results.

!!! tip ""
    If you only want to scan your environment for CVEs, you can also run `grype .pixi/envs/default` directly.

For Rust packages on conda-forge, building with `cargo-auditable` remains the current recommendation because it makes those shadow dependencies visible to downstream scanning tools. See the conda-forge [Rust packaging guide](https://conda-forge.org/docs/maintainer/example_recipes/rust) or the [conda-forge agent skill](https://prefix.dev/channels/skill-forge/packages/agent-skill-conda-forge) for the recommended recipe pattern.

**How to implement it**

Run Syft against the installed environment in CI or as part of your release review process, pass the conda and cargo-auditable catalogers explicitly, and feed the result directly into your vulnerability scanner. Generate a portable SBOM only when you need to archive or share that inventory with others.

## 5. Add Attestations When Publishing Your Own Artifacts

**What it is**

Pixi supports Sigstore-based attestations when publishing packages to [prefix.dev](https://prefix.dev).

**What it helps against**

Attestations are pieces of metadata about your package. Attestations are cryptographically signed to allow consumers to verify the content and origin of an attestation. Pixi and Rattler-Build generate build provenance attestations, which encode information about how and by whom a package was built. This is especially valuable when you operate your own channel or publish internal packages.

**How it works**

You can generate attestations directly during publish or upload an existing attestation in CI.

For example:

```bash
pixi publish --to https://prefix.dev/<channel-name> --generate-attestation
```

When using the lower-level upload command for prefix.dev, Pixi can also upload an existing attestation or generate one during CI:

```bash
pixi upload prefix --channel <channel-name> --generate-attestation dist/*.conda
```

One example of a channel that already uses package signing extensively is the [github-releases](https://prefix.dev/channels/github-releases) channel on prefix.dev (GitHub: [hunger/octoconda](https://github.com/hunger/octoconda)).

!!! note ""
    We are actively working on adding package signing to conda-forge, the most popular Conda channel.

!!! tip ""
    This is part of the broader conda ecosystem work to standardize attestation and signing. The current attestation work is captured in [CEP 27](https://conda.org/learn/ceps/cep-0027/), and broader package signing and Sigstore-serving work is still evolving in the ecosystem. We are also working on a proposal for serving this information on prefix.dev in [conda/ceps#142](https://github.com/conda/ceps/pull/142).

**How to implement it**

If you publish packages that others consume, generate attestations in CI by default and document how your consumers should verify them.

## Current Gaps And Practical Recommendation

Cross-ecosystem vulnerability matching for conda packages is still improving.

We are currently working on a PURL-related Conda Enhancement Proposal, [conda/ceps#63](https://github.com/conda/ceps/pull/63), that will make it easier to match conda-installed software against CVEs that are tracked in other ecosystems like PyPI. Until that work is standardized and widely implemented, direct scans of the already-installed environment with tools like Syft and Grype remain the most practical workaround.

For a broader view of the conda ecosystem work around regulatory readiness, SBOMs, CVE mapping, and auditable Rust binaries, see QuantCo's post, [Making the conda(-forge) ecosystem ready for cybersecurity regulations](https://tech.quantco.com/blog/conda-regulation-support).

If you want a conservative default posture, we recommend:

- commit and review `pixi.lock`;
- use `exclude-newer` for public channels;
- selectively bypass the delay only for trusted or urgent fixes;
- update or override dependencies when advisories land;
- scan the installed environment with Syft and your vulnerability scanner;
- generate attestations for packages you publish.
