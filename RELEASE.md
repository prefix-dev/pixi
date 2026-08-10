# Release process

Pixi is released in two steps: a version-bump PR, then a manually dispatched
build-and-publish workflow.

## 1. Open the release PR

Run:

```shell
pixi run release
```

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

Enable **dry-run** to build and package everything without signing, tagging or
publishing - useful for validating the build matrix from a branch.
