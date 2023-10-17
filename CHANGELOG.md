# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.6.0] - 2023-10-17

### Highlights
This release fixes some bugs and adds the `--cwd` option to the tasks.

### Details

#### Fixed
* Improve shell prompts by @ruben-arts in https://github.com/prefix-dev/pixi/pull/385 https://github.com/prefix-dev/pixi/pull/388
* Change `--frozen` logic to error when there is no lockfile by @ruben-arts in https://github.com/prefix-dev/pixi/pull/373
* Don't remove the '.11' from 'python3.11' binary file name by @ruben-arts in https://github.com/prefix-dev/pixi/pull/366

#### Changed
* Update `rerun` example to v0.9.1 by @ruben-arts in https://github.com/prefix-dev/pixi/pull/389

#### Added
* Add the current working directory (`--cwd`) in `pixi tasks` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/380

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.5.0...v0.6.0

## [0.5.0] - 2023-10-03

### Highlights

We rebuilt `pixi shell`, fixing the fact that your `rc` file would overrule the environment activation.

### Details

#### Fixed
* Change how `shell` works and make activation more robust by @wolfv in https://github.com/prefix-dev/pixi/pull/316
* Documentation: use quotes in cli by @pavelzw in https://github.com/prefix-dev/pixi/pull/367

#### Added
* Create or append to the `.gitignore` and `.gitattributes` files by @ruben-arts in https://github.com/prefix-dev/pixi/pull/359
* Add `--locked` and `--frozen` to getting an up-to-date prefix by @ruben-arts in https://github.com/prefix-dev/pixi/pull/363
* Documentation: improvement/update by @ruben-arts in https://github.com/prefix-dev/pixi/pull/355
* Example: how to build a docker image using `pixi` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/353 & https://github.com/prefix-dev/pixi/pull/365
* Update to the newest rattler by @baszalmstra in https://github.com/prefix-dev/pixi/pull/361
* Periodic `cargo upgrade --all --incompatible` by @wolfv in https://github.com/prefix-dev/pixi/pull/358

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.4.0...v0.5.0

## [0.4.0] - 2023-09-22

### Highlights

This release adds the start of a new cli command `pixi project` which will allow users to interact with the project configuration from the command line.

### Details

#### Fixed
* Align with latest rattler version `0.9.0` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/350

#### Added
* Add codespell (config, workflow) to catch typos + catch and fix some of those by @yarikoptic in https://github.com/prefix-dev/pixi/pull/329
* remove atty and use stdlib by @wolfv in https://github.com/prefix-dev/pixi/pull/337
* `xtsci-dist` to Community.md by @HaoZeke in https://github.com/prefix-dev/pixi/pull/339
* `ribasim` to Community.md by @Hofer-Julian in https://github.com/prefix-dev/pixi/pull/340
* `LFortran` to Community.md by @wolfv in https://github.com/prefix-dev/pixi/pull/341
* Give tip to resolve virtual package issue by @ruben-arts in https://github.com/prefix-dev/pixi/pull/348
* `pixi project channel add` subcommand by @baszalmstra and @ruben-arts in https://github.com/prefix-dev/pixi/pull/347

## New Contributors
* @yarikoptic made their first contribution in https://github.com/prefix-dev/pixi/pull/329
* @HaoZeke made their first contribution in https://github.com/prefix-dev/pixi/pull/339

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.3.0...v0.4.0

## [0.3.0] - 2023-09-11

### Highlights

This releases fixes a lot of issues encountered by the community as well as some awesome community contributions like the addition of `pixi global list` and `pixi global remove`.

### Details

#### Fixed

- Properly detect Cuda on linux using our build binaries, by @baszalmstra ([#290](https://github.com/mamba-org/rattler/pull/290))
- Package names are now case-insensitive, by @baszalmstra ([#285](https://github.com/mamba-org/rattler/pull/285))
- Issue with starts-with and compatibility operator, by @tdejager ([#296](https://github.com/mamba-org/rattler/pull/296))
- Lock files are now consistently sorted, by @baszalmstra ([#295](https://github.com/mamba-org/rattler/pull/295) & [#307](https://github.com/prefix-dev/pixi/pull/307))
- Improved xonsh detection and powershell env-var escaping, by @wolfv ([#307](https://github.com/mamba-org/rattler/pull/307))
- `system-requirements` are properly filtered by platform, by @ruben-arts ([#299](https://github.com/prefix-dev/pixi/pull/299))
- Powershell completion install script, by @chawyehsu ([#325](https://github.com/prefix-dev/pixi/pull/325))
- Simplified and improved shell quoting, by @baszalmstra ([#313](https://github.com/prefix-dev/pixi/pull/313))
- Issue where platform specific subdirs were required, by @baszalmstra ([#333](https://github.com/prefix-dev/pixi/pull/333))
- `thread 'tokio-runtime-worker' has overflowed its stack` issue, by @baszalmstra ([#28](https://github.com/idubrov/json-patch/pull/28))

#### Added

- Certificates from the OS certificate store are now used, by @baszalmstra ([#310](https://github.com/prefix-dev/pixi/pull/310))
- `pixi global list` and `pixi global remove` commands, by @cjfuller ([#318](https://github.com/prefix-dev/pixi/pull/318))

#### Changed

- `--manifest-path` must point to a `pixi.toml` file, by @baszalmstra ([#324](https://github.com/prefix-dev/pixi/pull/324))

## [0.2.0] - 2023-08-22

### Highlights
- Added `pixi search` command to search for packages, by @Wackyator. ([#244](https://github.com/prefix-dev/pixi/pull/244))
- Added target specific tasks, eg. `[target.win-64.tasks]`, by @ruben-arts. ([#269](https://github.com/prefix-dev/pixi/pull/269))
- Flaky install caused by the download of packages, by @baszalmstra. ([#281](https://github.com/prefix-dev/pixi/pull/281))

### Details
#### Fixed
- Install instructions, by @baszalmstra. ([#258](https://github.com/prefix-dev/pixi/pull/258))
- Typo in getting started, by @RaulPL. ([#266](https://github.com/prefix-dev/pixi/pull/266))
- Don't execute alias tasks, by @baszalmstra. ([#274](https://github.com/prefix-dev/pixi/pull/274))

#### Added
- Rerun example, by @ruben-arts. ([#236](https://github.com/prefix-dev/pixi/pull/236))
- Reduction of pixi's binary size, by @baszalmstra ([#256](https://github.com/prefix-dev/pixi/pull/256))
- Updated pixi banner, including webp file for faster loading, by @baszalmstra. ([#257](https://github.com/prefix-dev/pixi/pull/257))
- Set linguist attributes for `pixi.lock` automatically, by @spenserblack. ([#265](https://github.com/prefix-dev/pixi/pull/265))
- Contribution manual for pixi, by @ruben-arts. ([#268](https://github.com/prefix-dev/pixi/pull/268))
- GitHub issue templates, by @ruben-arts. ([#271](https://github.com/prefix-dev/pixi/pull/271))
- Links to prefix.dev in readme, by @tdejager. ([#279](https://github.com/prefix-dev/pixi/pull/279))

## [0.1.0] - 2023-08-11

As this is our first [Semantic Versioning](semver.org) release, we'll change from the prototype to the developing phase, as semver describes.
A 0.x release could be anything from a new major feature to a breaking change where the 0.0.x releases will be bugfixes or small improvements.

### Highlights
- Update to the latest [rattler](https://github.com/mamba-org/rattler/releases/tag/v0.7.0) version, by @baszalmstra. ([#249](https://github.com/prefix-dev/pixi/pull/249))
### Details
#### Fixed
- Only add shebang to activation scripts on `unix` platforms, by @baszalmstra. ([#250](https://github.com/prefix-dev/pixi/pull/250))
- Use official crates.io releases for all dependencies, by @baszalmstra. ([#252](https://github.com/prefix-dev/pixi/pull/252))

## [0.0.8] - 2023-08-01

### Highlights
- Much better error printing using `miette`, by @baszalmstra. ([#211](https://github.com/prefix-dev/pixi/pull/211))
- You can now use pixi on `aarch64-linux`, by @pavelzw.  ([#233](https://github.com/prefix-dev/pixi/pull/233))
- Use the Rust port of `libsolv` as the default solver, by @ruben-arts. ([#209](https://github.com/prefix-dev/pixi/pull/209))

### Details
#### Added
- Add mention to `condax` in the docs, by @maresb. ([#207](https://github.com/prefix-dev/pixi/pull/207))
- Add `brew` installation instructions, by @wolfv. ([#208](https://github.com/prefix-dev/pixi/pull/208))
- Add `activation.scripts` to the `pixi.toml` to configure environment activation, by @ruben-arts. ([#217](https://github.com/prefix-dev/pixi/pull/217))
- Add `pixi upload` command to upload packages to `prefix.dev`, by @wolfv. ([#127](https://github.com/prefix-dev/pixi/pull/127))
- Add more metadata fields to the `pixi.toml`, by @wolfv. ([#218](https://github.com/prefix-dev/pixi/pull/218))
- Add `pixi task list` to show all tasks in the project, by @tdejager. ([#228](https://github.com/prefix-dev/pixi/pull/228))
- Add `--color` to configure the colors in the output, by @baszalmstra. ([#243](https://github.com/prefix-dev/pixi/pull/243))
- Examples, [ROS2 Nav2](https://github.com/prefix-dev/pixi/tree/main/examples/ros2-nav2), [JupyterLab](https://github.com/prefix-dev/pixi/tree/main/examples/jupyterlab) and [QGIS](https://github.com/prefix-dev/pixi/tree/main/examples/qgis), by @ruben-arts.

#### Fixed
- Add trailing newline to `pixi.toml` and `.gitignore`, by @pavelzw. ([#216](https://github.com/prefix-dev/pixi/pull/216))
- Deny unknown fields and rename license-file in `pixi.toml`, by @wolfv. ([#220](https://github.com/prefix-dev/pixi/pull/220))
- Overwrite `PS1` variable when going into a `pixi shell`, by @ruben-arts. ([#201](https://github.com/prefix-dev/pixi/pull/201))

#### Changed
- Install environment when adding a dependency using `pixi add`, by @baszalmstra. ([#213](https://github.com/prefix-dev/pixi/pull/213))
- Improve and speedup CI, by @baszalmstra. ([#241](https://github.com/prefix-dev/pixi/pull/241))

## [0.0.7] - 2023-07-11

### Highlights
- Transitioned the `run` subcommand to use the `deno_task_shell` for improved cross-platform functionality. More details in the [Deno Task Runner documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner).
- Added an `info` subcommand to retrieve system-specific information understood by `pixi`.

### BREAKING CHANGES
- `[commands]` in the `pixi.toml` is now called `[tasks]`. ([#177](https://github.com/prefix-dev/pixi/pull/177))

### Details
#### Added
- The `pixi info` command to get more system information by @wolfv in ([#158](https://github.com/prefix-dev/pixi/pull/158))
- Documentation on how to use the cli by @ruben-arts in ([#160](https://github.com/prefix-dev/pixi/pull/160))
- Use the `deno_task_shell` to execute commands in `pixi run` by @baszalmstra in ([#173](https://github.com/prefix-dev/pixi/pull/173))
- Use new solver backend from rattler by @baszalmstra in ([#178](https://github.com/prefix-dev/pixi/pull/178))
- The `pixi command` command to the cli by @tdejager in ([#177](https://github.com/prefix-dev/pixi/pull/177))
- Documentation on how to use the `pixi auth` command by @wolfv in ([#183](https://github.com/prefix-dev/pixi/pull/183))
- Use the newest rattler 0.6.0 by @baszalmstra in ([#185](https://github.com/prefix-dev/pixi/pull/185))
- Build with pixi section to the documentation by @tdejager in ([#196](https://github.com/prefix-dev/pixi/pull/196))

#### Fixed
- Running tasks sequentially when using `depends_on` by @tdejager in ([#161](https://github.com/prefix-dev/pixi/pull/161))
- Don't add `PATH` variable where it is already set by @baszalmstra in ([#169](https://github.com/prefix-dev/pixi/pull/169))
- Fix README by @Hofer-Julian in ([#182](https://github.com/prefix-dev/pixi/pull/182))
- Fix Ctrl+C signal in `pixi run` by @tdejager in ([#190](https://github.com/prefix-dev/pixi/pull/190))
- Add the correct license information to the lockfiles by @wolfv in ([#191](https://github.com/prefix-dev/pixi/pull/191))


## [0.0.6] - 2023-06-30

### Highlights
Improving the reliability is important to us, so we added an integration testing framework, we can now test as close as possible to the CLI level using `cargo`.

### Details

#### Added
- An integration test harness, to test as close as possible to the user experience but in rust. ([#138](https://github.com/prefix-dev/pixi/pull/138), [#140](https://github.com/prefix-dev/pixi/pull/140), [#156](https://github.com/prefix-dev/pixi/pull/156))
- Add different levels of dependencies in preparation for `pixi build`, allowing `host-` and `build-` `dependencies` ([#149](https://github.com/prefix-dev/pixi/pull/149))

#### Fixed
- Use correct folder name on pixi init ([#144](https://github.com/prefix-dev/pixi/pull/144))
- Fix windows cli installer ([#152](https://github.com/prefix-dev/pixi/pull/152))
- Fix global install path variable ([#147](https://github.com/prefix-dev/pixi/pull/147))
- Fix macOS binary notarization ([#153](https://github.com/prefix-dev/pixi/pull/153))

## [0.0.5] - 2023-06-26

Fixing Windows installer build in CI. ([#145](https://github.com/prefix-dev/pixi/pull/145))

## [0.0.4] - 2023-06-26

### Highlights

A new command, `auth` which can be used to authenticate the host of the package channels.
A new command, `shell` which can be used to start a shell in the pixi environment of a project.
A refactor of the `install` command which is changed to `global install` and the `install` command now installs a pixi project if you run it in the directory.
Platform specific dependencies using `[target.linux-64.dependencies]` instead of `[dependencies]` in the `pixi.toml`

Lots and lots of fixes and improvements to make it easier for this user, where bumping to the new version of [`rattler`](https://github.com/mamba-org/rattler/releases/tag/v0.4.0)
helped a lot.

### Details

#### Added

- Platform specific dependencies and helpful error reporting on `pixi.toml` issues([#111](https://github.com/prefix-dev/pixi/pull/111))
- Windows installer, which is very useful for users that want to start using pixi on windows. ([#114](https://github.com/prefix-dev/pixi/pull/114))
- `shell` command to use the pixi environment without `pixi run`. ([#116](https://github.com/prefix-dev/pixi/pull/116))
- Verbosity options using `-v, -vv, -vvv` ([#118](https://github.com/prefix-dev/pixi/pull/118))
- `auth` command to be able to login or logout of a host like `repo.prefix.dev` if you're using private channels. ([#120](https://github.com/prefix-dev/pixi/pull/120))
- New examples: CPP sdl: [#121](https://github.com/prefix-dev/pixi/pull/121), Opencv camera calibration [#125](https://github.com/prefix-dev/pixi/pull/125)
- Apple binary signing and notarization. ([#137](https://github.com/prefix-dev/pixi/pull/137))

#### Changed

- `pixi install` moved to `pixi global install` and `pixi install` became the installation of a project using the `pixi.toml` ([#124](https://github.com/prefix-dev/pixi/pull/124))

#### Fixed

- `pixi run` uses default shell ([#119](https://github.com/prefix-dev/pixi/pull/119))
- `pixi add` command is fixed. ([#132](https://github.com/prefix-dev/pixi/pull/132))

- Community issues fixed: [#70](https://github.com/prefix-dev/pixi/issues/70), [#72](https://github.com/prefix-dev/pixi/issues/72),  [#90](https://github.com/prefix-dev/pixi/issues/90), [#92](https://github.com/prefix-dev/pixi/issues/92), [#94](https://github.com/prefix-dev/pixi/issues/94), [#96](https://github.com/prefix-dev/pixi/issues/96)
