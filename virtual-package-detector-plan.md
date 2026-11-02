# Local virtual package detector example

## Scope

Add a complete local example based on [virtual-package-detector-demo](https://github.com/Hofer-Julian/virtual-package-detector-demo).

- Build two `noarch` Python packages with `pixi-build-python`.
- `username-length-detect` reports the username's character count as `__username_length`.
- `long-username` requires `__username_length >=10` and prints its name.
- Publish both packages to a local channel. List conda-forge alongside it for Python.
- Support `linux-64`, `linux-aarch64`, `osx-64`, `osx-arm64`, and `win-64` in one consumer.
- No fallback platform, package variants, hosted channel, or channel-relations prerequisite.
- Keep the docs concise and in Julian's voice. No post-implementation walkthrough.

## Detector dependency resolution

Extend Pixi's detector environment provider to use the effective solve's explicitly configured channels and their channel relations for dependencies.

- Keep the detector package pinned to its registering channel.
- Preserve configured channel priority and per-environment channel scope.
- Use only built-in virtual packages when resolving the detector, preventing recursion.
- Preserve consent, disabled link scripts, environment digesting, and caching.
- Keep the change in Pixi using the existing provider API; no Rattler dependency change.
- Add deterministic integration coverage proving that an executed dependency comes from a separate configured channel, configured priority is honored, and an unregistered same-name package cannot replace the origin detector.

## Example and documentation

- Put the runnable sources under `docs/source_files/pixi_workspaces/virtual_package_detector/`: `detector/`, `package/`, `consumer/pixi.toml`, and `index-config.toml`.
- Use only Python's standard library in the commands. The detector must not depend on the virtual package it reports.
- Register `username-length-detect = ["__username_length"]` with `rattler-index`.
- Ignore generated local channels, environments, and Python caches.
- Add `docs/tutorials/virtual_package_detectors.md`, using source includes instead of duplicate code.
- Cover building, indexing, trust, consumer installation, and command-scoped Nushell overrides.
- Distinguish declared values used for locking from detected host values used for installation.
- Add tutorial navigation and a link from the existing detector section. Document the extended dependency-channel scope in the configuration reference.

## Verification

1. Run the new regression against the existing CLI before the fix, then build the changed workspace and run detector integration coverage.
2. Build both real Python packages and index a fresh local channel in an isolated directory with separate Pixi home, cache, and consent configuration.
3. Inspect package metadata for the actual `__username_length >=10` dependency and repodata for registration.
4. Run the detector through Pixi without an override, using a controlled 12-character username. Confirm its report and execution state through `pixi info --json`.
5. Exercise the trust prompt in a terminal and save the decision for the workspace.
6. Install and run successfully at 12; check the exact command output and the inclusive boundary at 10.
7. Confirm `pixi install --locked` rejects 9 on fresh and installed consumer environments without changing the lockfile. Overrides alone do not prove detector execution.
8. Run relevant formatting and lint, and `pixi run -e docs build-docs`. Inspect the rendered tutorial and navigation in a browser, then close the tab.
9. Invoke `deslop`. Remove temporary verification files and generated artifacts; do not add an activity-log changelog entry.
10. Before landing: invoke the `adversarial-review` skill and follow it. Recheck paths changed by fixes.

Report exactly which platforms were exercised. Do not infer five-platform runtime verification from `noarch` artifacts or successful solves alone.
