# Supply Chain Security

Pixi helps reduce supply chain risk in a few different ways:

- it resolves environments into a lock file, so installs are based on explicit artifacts instead of whatever happens to be newest at install time;
- it can exclude recently published packages with [`exclude-newer`](reference/pixi_manifest.md#exclude-newer-optional);
- it lets you react to advisories by constraining or overriding affected dependencies;
- it supports generating and uploading Sigstore attestations when publishing to prefix.dev.

At the same time, Pixi does not try to be a full vulnerability scanner for conda environments. Today, our preferred way to inspect a Pixi-managed environment for CVE analysis is to generate an SBOM from the installed environment with [Syft](https://github.com/anchore/syft).

## Reproducible Dependency Resolution

The first layer of supply chain security is reproducibility.

Pixi records the resolved environment in `pixi.lock`, including the exact package artifacts that were selected. That gives you a stable review surface in code review and makes unexpected dependency changes visible when the lock file changes.

To review lock file changes between commits in a human-readable way, you can use [`pixi-diff`](integration/extensions/pixi_diff.md) directly or integrate the output into CI with [`pixi-diff-to-markdown`](integration/ci/updates_github_actions.md).

Using the following command, you can generate a readable overview of what changed between an older lock file and your current one:

```bash
# bash / zsh
pixi diff --before <(git show HEAD~20:pixi.lock) --after pixi.lock | pixi diff-to-markdown

# fish
pixi diff --before (git show HEAD~20:pixi.lock | psub) --after pixi.lock | pixi diff-to-markdown
```

## Delaying Fresh Uploads With `exclude-newer`

One practical defense against compromised package releases is to avoid resolving against packages uploaded very recently.

For security-focused setups, relative durations are usually the most practical configuration. A common pattern is to keep a delay for public channels, while allowing trusted internal channels or specific emergency fixes through immediately. See [`exclude-newer`](reference/pixi_manifest.md#exclude-newer-optional) for the supported manifest field:

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

This is useful when you want a conservative trust window for public ecosystems, but still need to selectively force-include trusted or urgent fixes for a channel or dependency.

!!! tip "CEP for `upload_timestamp` in repodata"
    There is also an in-progress proposal, [conda/ceps#154](https://github.com/conda/ceps/pull/154), to include upload timestamps in `repodata.json`. If adopted, that would let tools consume channel-provided upload times directly and harden this workflow against spoofed timestamp entries from conda-forge itself.

## Responding To Vulnerability Advisories

When a CVE affects one of your dependencies, there the best way to respond is to update your dependency to a non-vulnerable version.
In those cases, you might need to decrease the package-specific `exclude-newer` as mentioned above.

It can happen that another dependency has an upper bound preventing pixi from solving this environment with the updated dependency.
To mitigate this, you can override certain dependencies using [`dependency-overrides`](advanced/override.md).

!!!note "PyPi-only for now"
    We are planning to support a similar feature like this for Conda packages as well. For more information, see [pixi#4891](https://github.com/prefix-dev/pixi/issues/4891).

```toml
[pypi-options.dependency-overrides]
# force all packages to depend on urllib3 >=2.2.2
urllib3 = ">=2.2.2"
```

These controls are complementary to `exclude-newer`: `exclude-newer` reduces exposure to newly uploaded artifacts, while constraints and overrides help you respond once a vulnerable version is already known.

## SBOM Generation Today

To analyze a Pixi-managed conda environment for vulnerabilities today, our preferred workflow is to generate an SBOM from the installed environment with [Syft](https://github.com/anchore/syft).

For a default workspace environment, that usually means scanning the environment directory directly:

```bash
syft .pixi/envs/default
```

This matters because scanning the installed environment captures what is actually present on disk, not just what was requested in the manifest.

Syft is especially useful as the inventory step in that process: it catalogs the installed software and produces an SBOM that can then be fed into the rest of your vulnerability workflow.

For Rust packages on conda-forge, building with `cargo-auditable` remains the current recommendation because it makes those shadow dependencies visible to downstream scanning tools. See the conda-forge [Rust packaging guide](https://conda-forge.org/docs/maintainer/example_recipes/rust) or the [conda-forge agent skill](https://prefix.dev/channels/skill-forge/packages/agent-skill-conda-forge) for the recommended recipe pattern.

## Package Signing And Attestations

Pixi already supports Sigstore-based attestations when publishing packages to [prefix.dev](https://prefix.dev).

For example:

```bash
pixi publish --to https://prefix.dev/<channel-name> --generate-attestation
```

When using the lower-level upload command for prefix.dev, Pixi can also upload an existing attestation or generate one during CI:

```bash
pixi upload prefix --channel <channel-name> --generate-attestation dist/*.conda
```

One example of a channel that already uses package signing extensively is the [github-releases](https://prefix.dev/channels/github-releases) channel on prefix.dev (GitHub: [hunger/octoconda](https://github.com/hunger/octoconda)).

!!!note ""
    We are actively working on adding package signing to conda-forge, the most popular Conda channel.

!!!tip ""
    This is part of the broader conda ecosystem work to standardize attestation and signing. The current attestation work is captured in [CEP 27](https://conda.org/learn/ceps/cep-0027/), and broader package signing and Sigstore-serving work is still evolving in the ecosystem. We are also working on a proposal for serving this information on prefix.dev in [conda/ceps#142](https://github.com/conda/ceps/pull/142).

## Work In Progress

Cross-ecosystem vulnerability matching for conda packages is still improving.

We are currently working on a PURL-related Conda Enhancement Proposal, [conda/ceps#63](https://github.com/conda/ceps/pull/63), that will make it easier to match conda-installed software against CVEs that are tracked in other ecosystems like PyPi.
Today, generating an SBOM from the already-installed environment with tools like Syft is still the most practical workaround for that gap.

For a broader view of the conda ecosystem work around regulatory readiness, SBOMs, CVE mapping, and auditable Rust binaries, see QuantCo's post, [Making the conda(-forge) ecosystem ready for cybersecurity regulations](https://tech.quantco.com/blog/conda-regulation-support).

Until that work is standardized and widely implemented, the safest approach is:

- keep lock files under review;
- use `exclude-newer` where a delayed trust window makes sense;
- update your dependencies when advisories land;
- generate an SBOM from the installed environments with Syft as part of your security review workflow.
