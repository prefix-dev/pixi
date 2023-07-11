# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.0.7] - 2023-07-11

### Highlights
- Transitioned the `run` subcommand to use the `deno_task_shell` for improved cross-platform functionality. More details in the [Deno Task Runner documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner).
- Added an `info` subcommand to retrieve system-specific information understood by `pixi`.

### BRAKING CHANGES
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
