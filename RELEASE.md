# Release process

Pixi is released in two steps: a version-bump PR, then a manually dispatched
build-and-publish workflow.

## 1. Open the release PR

The `release-prepare` environment supplies Rust/Cargo, Python, Git, the GitHub
CLI, and the version/changelog tools. It shares the Rust compiler feature with
the development and backend-release environments without enabling
development-only activation settings.

Authenticate with `pixi run -e release-prepare gh auth login` and configure your
Git author identity before running:

```shell
pixi run release
```

The explicit environment form is `pixi run -e release-prepare release`.

This branches from `prefix-dev/pixi@main`, bumps the version in
`crates/pixi/Cargo.toml` and the other tracked files, regenerates the
`CHANGELOG.md` section with git-cliff, updates the lock files, and opens a PR.
Edit the `✨ Highlights` section of `CHANGELOG.md` when prompted.

Review and merge the PR. `crates/pixi/Cargo.toml` is the single source of truth
for the release version from here on.

## 2. Run the Release workflow

Once the bump PR is on `main`, go to the
[Release workflow](https://github.com/prefix-dev/pixi/actions/workflows/release.yml)
and press **Run workflow** on `main`.

The workflow:

1. Reads the version from `crates/pixi/Cargo.toml`.
2. Builds, signs and packages every target (archives, raw binaries, and the
   Windows MSI).
3. Pushes the `vX.Y.Z` tag only after every build succeeds.
4. Publishes the GitHub release with notes taken from `CHANGELOG.md` and a
   `sha256.sum` of all artifacts.
5. Dispatches the docs deploy and the WinGet publish.

Release builds emit warnings without failing on them, including in PGO builds.
CI and lint jobs enforce warning-free code. Linux targets build through
`cargo-zigbuild`.

Enable **dry-run** to build and package everything without signing, tagging or
publishing - useful for validating the build matrix from a branch.
