# Supply Chain Security

Supply chain security starts with a simple assumption: even when your own code is correct, the packages you install can still become the attack path.

Typical risks include:

- compromised or tampered artifacts;
- hostile or hijacked upstream releases;
- newly published packages that later turn out to be malicious or broken;
- known-vulnerable dependencies that stay in your environment because nothing forces an update;
- poor visibility into what actually changed between one environment revision and the next.

Pixi does not eliminate these risks, and it does not include a vulnerability scanner for conda environments. What it does provide is a set of practical controls that help you reduce exposure, review changes more clearly, and respond faster when something goes wrong.

This is the security model we recommend, step by step:

1. make dependency resolution reproducible and reviewable;
2. delay very fresh uploads when a cooldown is appropriate;
3. respond to advisories by constraining or overriding dependencies;
4. treat package installation and activation hooks as code execution surfaces;
5. scan the installed environment with tooling that understands what is actually on disk;
6. add attestations when you publish your own artifacts.

## 1. Make Dependency Resolution Reproducible

Keep `pixi.lock` under version control, review lock file diffs in pull requests, and treat unexpected artifact changes as a security-relevant event. Pixi records the fully resolved environment in `pixi.lock`, including the exact artifacts that were selected, so future installs use those locked artifacts instead of "whatever is newest right now". That reduces the risk of silent dependency drift and gives you a stable review surface: if a dependency changes, the lock file changes too.

To review lock file changes between commits in a human-readable way, you can use [`pixi-diff`](integration/extensions/pixi_diff.md) directly or integrate the output into CI with [`pixi-diff-to-markdown`](integration/ci/updates_github_actions.md).

For example:

```bash
# bash / zsh
pixi diff --before <(git show HEAD~20:pixi.lock) --after pixi.lock | pixi diff-to-markdown

# fish
pixi diff --before (git show HEAD~20:pixi.lock | psub) --after pixi.lock | pixi diff-to-markdown
```

## 2. Delay Fresh Uploads With `exclude-newer`

Use [`exclude-newer`](reference/pixi_manifest.md#exclude-newer-optional) to put a short delay on packages from public channels by default, then carve out exceptions only for artifacts you control or urgent security fixes that you have explicitly reviewed. This is a practical defense against compromised releases, rushed hostile uploads, or ecosystem incidents where a bad package is published and only detected shortly afterwards.

The delay is useful even when public ecosystems already run security checks, because those checks are often asynchronous. Package indexes may publish first and only then run third-party analysis that later flags a release. A cooldown of a few days gives those external checks, user reports, and ecosystem response processes time to surface problems before your resolver consumes the new artifact. Pixi can apply that delay across your workspace and still let you opt specific channels or packages back in immediately when needed, which is useful when you want a conservative trust window for public ecosystems while still allowing trusted internal channels or urgent fixes through without delay.

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

!!! tip "CEP for `upload_timestamp` in repodata"
    There is also an in-progress proposal, [conda/ceps#154](https://github.com/conda/ceps/pull/154), to include upload timestamps in `repodata.json`. If adopted, that would let tools consume channel-provided upload times directly and harden this workflow against spoofed timestamp entries from conda-forge itself.
    Once this CEP is implemented, Pixi will switch the behavior of `exclude-newer` to use the `upload_timestamp` instead of the `timestamp`.

## 3. Respond To Advisories With Constraints And Overrides

When an advisory lands, update to the fixed version first. If the fix is fresh, that can also mean temporarily relaxing `exclude-newer` so you can adopt the security release immediately. If another dependency prevents the solver from reaching the non-vulnerable version, Pixi supports [`dependency-overrides`](advanced/override.md) for PyPI packages.

This is how you respond when a vulnerable version is already known, especially if transitive dependency bounds would otherwise keep you stuck on an affected release. Overrides let you force dependency requirements during resolution so the solver can select a safe version even if upstream packages have not caught up yet. They are complementary to `exclude-newer`: `exclude-newer` reduces exposure to newly uploaded artifacts, while constraints and overrides help you respond once a vulnerable version is already known.

!!! note "PyPI-only for now"
    We are planning to support a similar feature like this for Conda packages as well. For more information, see [pixi#4891](https://github.com/prefix-dev/pixi/issues/4891).

```toml
[pypi-options.dependency-overrides]
# force all packages to depend on urllib3 >=2.2.2
urllib3 = ">=2.2.2"
```

Do not trust overrides blindly. In the best case, maintainers publish a patch release on the same minor version you are already using, so the compatibility risk stays small. In practice, the available fix can sometimes be in a newer major version, and forcing that version through an override can break compatibility with other packages. Treat overrides as a short-term mitigation: apply the smallest possible change, test the affected environment, and remove the override once upstream metadata or upstream releases make it unnecessary.

## 4. Treat Package Hooks As Code Execution

Treat `pixi shell`, `pixi run`, `pixi shell-hook`, and any automation around them as code execution boundaries, not just environment setup commands. Conda packages can carry executable hooks in addition to files and metadata. Two especially relevant cases are:

- [`post-link` scripts](reference/pixi_configuration.md#run-post-link-scripts), which run during installation if explicitly enabled;
- [activation scripts](workspace/environment.md#activation), which are run during environment activation.

This is a different class of supply-chain risk from "is this version vulnerable?": package installation or activation itself can become the arbitrary code execution event. Pixi disables `post-link` scripts by default, so package-provided shell or batch scripts are not allowed to run during installation unless you explicitly enable that functionality. Activation scripts are different: they are part of normal conda environment activation and are currently run by default when you use `pixi shell`, `pixi run`, or `pixi shell-hook`. That means a malicious package can execute code at activation time.

!!! tip "Disable activation scripts in Pixi"
    We plan to add an option to disable shell activation scripts and allow JSON-style activations only. Track progress in [pixi#4889](https://github.com/prefix-dev/pixi/issues/4889).

This also affects `direnv` integrations. The documented [`direnv` setup](integration/third_party/direnv.md) uses `watch_file pixi.lock`, which means a lock file change causes `direnv` to re-run `pixi shell-hook`. If the new lock file introduces a package with a malicious activation script, switching to that lock file can trigger the same arbitrary code execution without a fresh manual approval.

!!! tip "`require_allowed` in `direnv`"
    There is an upstream `direnv` pull request, [direnv#1530](https://github.com/direnv/direnv/pull/1530), that adds `require_allowed pixi.toml pixi.lock`. Once released, that can be used to force a fresh `direnv allow` when the manifest or lock file changes.

Keep `post-link` scripts disabled unless you have a concrete package that requires them and you have reviewed that behavior. If you use `direnv`, be aware that `watch_file pixi.lock` improves convenience but also lowers the friction for activation-time code execution after dependency changes. Re-introduce an approval step on lock file changes as soon as your `direnv` version supports it.

## 5. Scan The Installed Environment Directly

For CVE analysis today, our preferred workflow is to scan the installed Pixi-managed environment directly with [Syft](https://github.com/anchore/syft) and a vulnerability scanner such as [Grype](https://github.com/anchore/grype). Run that scan in CI or as part of your release review process, and generate a portable SBOM only when you need to archive the inventory or share it with someone else.

This improves visibility because it tells you what is actually present on disk, which is what your security tooling ultimately needs to analyze. That is especially relevant in the conda ecosystem, where cross-ecosystem vulnerability matching is still improving. Today, a file-system scan of the installed environment is often the most practical way to bridge that gap. Instead of scanning only what was requested in `pixi.toml`, you scan the concrete environment directory. We recommend enabling the conda and auditable Rust catalogers explicitly so the behavior stays consistent across different scan targets:

```bash
syft .pixi/envs/default \
  --select-catalogers=+conda-meta-cataloger,+cargo-auditable-binary-cataloger \
  --output syft-json=syft-output.json
```

That output can then be fed to Grype for CVE scanning:

```bash
grype syft-output.json
```

Syft will detect conda packages from `conda-meta` when scanning a filesystem location like `.pixi/envs/default`, but it does not always do so by default when scanning a container image that contains a conda environment. Passing the catalogers explicitly avoids that surprise.

If you want to continue straight into vulnerability analysis, prefer feeding Syft's own output into your scanner instead of converting through CycloneDX first. In practice, format conversion can lose information, and scanning a CycloneDX export can produce different results from scanning Syft's native output directly.

For conda packages specifically, Syft currently tends to emit CPEs but not PURLs. That means Grype may need to be configured to match on CPEs if you want useful conda vulnerability results.

!!! tip ""
    If you only want to scan your environment for CVEs, you can also run `grype .pixi/envs/default` directly.

For Rust packages on conda-forge, building with `cargo-auditable` remains the current recommendation because it makes those shadow dependencies visible to downstream scanning tools. See the conda-forge [Rust packaging guide](https://conda-forge.org/docs/maintainer/example_recipes/rust) or the [conda-forge agent skill](https://prefix.dev/channels/skill-forge/packages/agent-skill-conda-forge) for the recommended recipe pattern.

## 6. Add Attestations When Publishing Your Own Artifacts

If you publish packages that others consume, generate attestations in CI by default and document how your consumers should verify them. Pixi supports Sigstore-based attestations when publishing packages to [prefix.dev](https://prefix.dev). Attestations are cryptographically signed metadata about your package, and Pixi and Rattler-Build generate build provenance attestations that encode information about how and by whom a package was built. This is especially valuable when you operate your own channel or publish internal packages.

You can generate attestations directly during publish or upload an existing attestation in CI.

For more background on Sigstore attestations in the conda ecosystem, see the [Rattler-Build Sigstore documentation](https://rattler-build.prefix.dev/latest/sigstore/).

For example:

```bash
pixi publish --to https://prefix.dev/<channel-name> --generate-attestation
```

When using the lower-level upload command for prefix.dev, Pixi can also upload an existing attestation or generate one during CI:

```bash
pixi upload prefix --channel <channel-name> --generate-attestation dist/*.conda
```

Consumers can then validate those attestations with Sigstore tooling. For example, with the GitHub CLI:

```bash
gh attestation verify my-package-0.1.0-h123_0.conda \
  --owner my-org \
  --predicate-type "https://schemas.conda.org/attestations-publish-1.schema.json"
```

Or with `cosign`:

```bash
cosign verify-blob \
  --certificate-identity-regexp "https://github.com/my-org/.*" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  my-package-0.1.0-h123_0.conda
```

One example of a channel that already uses package signing extensively is the [github-releases](https://prefix.dev/channels/github-releases) channel on prefix.dev (GitHub: [hunger/octoconda](https://github.com/hunger/octoconda)).

!!! note ""
    We are actively working on adding package signing to conda-forge, the most popular Conda channel.

!!! tip "Serving Sigstore Attestations in Conda Channels"
    This is part of the broader conda ecosystem work to standardize attestation and signing. The current attestation work is captured in [CEP 27](https://conda.org/learn/ceps/cep-0027/), and broader package signing and Sigstore-serving work is still evolving in the ecosystem. We are also working on a proposal for serving this information on prefix.dev in [conda/ceps#142](https://github.com/conda/ceps/pull/142).

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
