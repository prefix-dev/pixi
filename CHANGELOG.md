# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

### [0.54.2] - 2025-09-08
#### Added

- Add cancellation support to command dispatcher by @baszalmstra in [#4501](https://github.com/prefix-dev/pixi/pull/4501)
- Support `win-32` emulation on `win-64` by @baszalmstra in [#4531](https://github.com/prefix-dev/pixi/pull/4531)

#### Changed

- Include version information in `self-update --dry-run` by @nicoddemus in [#4513](https://github.com/prefix-dev/pixi/pull/4513)

#### Documentation

- Fix task arg example by @Hofer-Julian in [#4521](https://github.com/prefix-dev/pixi/pull/4521)
- Add `pixi-build-ros` tutorial by @ruben-arts in [#4507](https://github.com/prefix-dev/pixi/pull/4507)

#### Fixed

- Clear progress bar after install by @baszalmstra in [#4509](https://github.com/prefix-dev/pixi/pull/4509)
- Rebuild if dependencies changed by @baszalmstra in [#4511](https://github.com/prefix-dev/pixi/pull/4511)
- `pixi build` stop generating empty directories by @Hofer-Julian in [#4520](https://github.com/prefix-dev/pixi/pull/4520)
- Documentation link and error in `pixi init` by @ruben-arts in [#4530](https://github.com/prefix-dev/pixi/pull/4530)

#### Performance

- Early out on installer file not containing the pixi uv installer by @ruben-arts in [#4525](https://github.com/prefix-dev/pixi/pull/4525)

#### New Contributors
* @nicoddemus made their first contribution in [#4513](https://github.com/prefix-dev/pixi/pull/4513)

### [0.54.1] - 2025-09-03
#### ✨ Highlights

Small improvements and bug fixes.

#### Changed

- Fix `--frozen` and `--locked` collision by @tdejager in [#4488](https://github.com/prefix-dev/pixi/pull/4488)
- Cache backend discovery by @baszalmstra in [#4492](https://github.com/prefix-dev/pixi/pull/4492)


#### Fixed

- Colors being escaped in cli output by @ruben-arts in [#4490](https://github.com/prefix-dev/pixi/pull/4490)
- Print build log by default by @ruben-arts in [#4495](https://github.com/prefix-dev/pixi/pull/4495)
- Panic on moving package source location by @Hofer-Julian in [#4496](https://github.com/prefix-dev/pixi/pull/4496)


### [0.54.0] - 2025-09-01
#### ✨ Highlights

You can now use `pixi global tree` to visualize the dependency tree of a global environment.

And you can install subsets of packages now works, for both conda and pypi packages:
  ```bash
  # Define which packages you want to install and which you want to skip.
  pixi install --only packageA --only packageB --skip packageC
  
  # Using this modified environment without updating it again can be done with:
  pixi run --as-is my_command
  pixi shell --as-is  
  ```

#### Breaking Change
Only for users using `preview = ["pixi-build"]`:
In [#4410](https://github.com/prefix-dev/pixi/pull/4410) we've made `package.name` optional. e.g.
```toml
[package]
name = "my-package" # This is now optional
version = "0.1.0" # This is now optional
```
Soon, the backends will be able to automatically get those values from `pyproject.toml`, `Cargo.toml`, `package.xml` etc.
However, this results in the lockfiles not being `--locked` anymore. 
Running `pixi lock` or `pixi update` should fix this!

#### Added

- Add new `pixi global tree` command by @Carbonhell in [#4427](https://github.com/prefix-dev/pixi/pull/4427)
- Add the option for multiple `--only` flags. by @tdejager in [#4477](https://github.com/prefix-dev/pixi/pull/4477) & [#4404](https://github.com/prefix-dev/pixi/pull/4404)

#### Changed

- Ability to ignore `pypi` packages during installation by @tdejager in [#4456](https://github.com/prefix-dev/pixi/pull/4456)
- Move integration tests to pixi crate by @tdejager in [#4472](https://github.com/prefix-dev/pixi/pull/4472)

#### Documentation

- Extend `pixi global` tutorial with source dependencies by @Hofer-Julian in [#4407](https://github.com/prefix-dev/pixi/pull/4407)
- Fix `pixi-build` getting started doc by @Tobias-Fischer in [#4415](https://github.com/prefix-dev/pixi/pull/4415)
- Remove unused footnote in `lockfile.md` by @ZhenShuo2021 in [#4439](https://github.com/prefix-dev/pixi/pull/4439)

#### Fixed

- Relative windows --paths by @baszalmstra in [#4395](https://github.com/prefix-dev/pixi/pull/4395)
- Auth middleware should be added after mirror rewrite by @maccam912 in [#4399](https://github.com/prefix-dev/pixi/pull/4399)
- Occasionally hangs on exit by @baszalmstra in [#4409](https://github.com/prefix-dev/pixi/pull/4409)
- Ensure we only create pypi prefix once by @baszalmstra in [#4416](https://github.com/prefix-dev/pixi/pull/4416)
- Diagnostic source of solve error was not propagated by @baszalmstra in [#4417](https://github.com/prefix-dev/pixi/pull/4417)
- Warn about no pinning strategy for unused features by @kilian-hu in [#4065](https://github.com/prefix-dev/pixi/pull/4065)
- Load lock file with dependency override by @HernandoR in [#4419](https://github.com/prefix-dev/pixi/pull/4419)
- Adapt schema for `[workspace.target.OS.build-variants]` by @Hofer-Julian in [#4445](https://github.com/prefix-dev/pixi/pull/4445)
- Create virtual PyPI packages by @tdejager in [#4469](https://github.com/prefix-dev/pixi/pull/4469)
- Pixi global update shortcuts by @Hofer-Julian in [#4463](https://github.com/prefix-dev/pixi/pull/4463)
- Environment never recovering after subset install by @tdejager in [#4479](https://github.com/prefix-dev/pixi/pull/4479)

#### Refactor

- Move pixi crate into crates/pixi_cli by @haecker-felix in [#4377](https://github.com/prefix-dev/pixi/pull/4377)
- Drop dedicated cli module from pixi_cli and move everything into /src by @haecker-felix in [#4398](https://github.com/prefix-dev/pixi/pull/4398)
- Move global into new pixi_global crate by @haecker-felix in [#4388](https://github.com/prefix-dev/pixi/pull/4388)
- Move task into new pixi_task crate by @haecker-felix in [#4401](https://github.com/prefix-dev/pixi/pull/4401)
- BREAKING: make name optional by @baszalmstra in [#4410](https://github.com/prefix-dev/pixi/pull/4410)

#### Removed

- Remove outdated comment about fast channel by @lucascolley in [#4471](https://github.com/prefix-dev/pixi/pull/4471)

#### New Contributors
* @ZhenShuo2021 made their first contribution in [#4439](https://github.com/prefix-dev/pixi/pull/4439)
* @Tobias-Fischer made their first contribution in [#4415](https://github.com/prefix-dev/pixi/pull/4415)
* @kilian-hu made their first contribution in [#4065](https://github.com/prefix-dev/pixi/pull/4065)
* @maccam912 made their first contribution in [#4399](https://github.com/prefix-dev/pixi/pull/4399)

### [0.53.0] - 2025-08-19
#### ✨ Highlights

- Big cleanup of the CLI flags, mainly `--frozen`, `--locked`, and `--no-install`.
- Added `--as-is` to `pixi run/shell` to run the command without installing the dependencies or touching the lockfile.
- Support the Bash shell on Windows using `pixi shell`.
- Pixi build can now support `package.build.source.path = "some/path"` to use a different source root for the build.

#### ⚠️ Breaking Change

We've removed `--no-lockfile-update` and replaced it with `--no-install --frozen`.
On `pixi run/shell` you can use `--as-is` to run the command without installing the dependencies or touching the lockfile.

#### Added

- Add `--as-is` as a shorthand for `--no-install --frozen` by @tdejager in [#4357](https://github.com/prefix-dev/pixi/pull/4357)
- Add profiling profile by @ruben-arts in [#4376](https://github.com/prefix-dev/pixi/pull/4376)

#### Changed

- Infer package name for source package with `pixi global install` by @Hofer-Julian in [#4340](https://github.com/prefix-dev/pixi/pull/4340)
- Add `pixi --list` to view all the commands (pixi-extensions + built-in commands) by @mrswastik-robot in [#4307](https://github.com/prefix-dev/pixi/pull/4307)
- Use log level info for build backends by @pavelzw in [#4354](https://github.com/prefix-dev/pixi/pull/4354)
- Use `pixi build` with non-prefix channels, through fixing `run-exports` fetching by @remimimimimi in [#4179](https://github.com/prefix-dev/pixi/pull/4179)
- Alternative source root for build by @remimimimimi in [#4240](https://github.com/prefix-dev/pixi/pull/4240)
- Add `--build-platform` to `pixi build` by @baszalmstra in [#4298](https://github.com/prefix-dev/pixi/pull/4298)
- Support `Bash` on Windows using 'pixi shell' by @mwiebe in [#3981](https://github.com/prefix-dev/pixi/pull/3981)
- Move `build.channels` to `build.backend.channels` by @nichmor in [#4361](https://github.com/prefix-dev/pixi/pull/4361)
- Return error on build dispatch panic by @tdejager in [#4382](https://github.com/prefix-dev/pixi/pull/4382)

#### Documentation

- Add document for proxy-config table by @gzm55 in [#4367](https://github.com/prefix-dev/pixi/pull/4367)

#### Fixed

- Cargo-machete action 0.9.1 by @bnjbvr in [#4368](https://github.com/prefix-dev/pixi/pull/4368)
- Preserve comments when inserting dependency to manifest by @baszalmstra in [#4370](https://github.com/prefix-dev/pixi/pull/4370)
- Now cache miss when a pypi no-binary, no-build was found by @tdejager in [#4362](https://github.com/prefix-dev/pixi/pull/4362)
- Resolve shell quoting issue with pixi run commands by @chrisburr in [#4352](https://github.com/prefix-dev/pixi/pull/4352)
- Allow dots in global environment names by @baszalmstra in [#4374](https://github.com/prefix-dev/pixi/pull/4374)
- Improve `infer_package_name_from_spec` by @Hofer-Julian in [#4378](https://github.com/prefix-dev/pixi/pull/4378)
- Replace `build.configuration` with `build.config` by @ruben-arts in [#4380](https://github.com/prefix-dev/pixi/pull/4380)
- Pass `run-dependencies` and `run-exports` to packages by @baszalmstra in [#4373](https://github.com/prefix-dev/pixi/pull/4373)

#### Refactor

- Remove manual conflicts check for `--frozen` & `--locked` by @tdejager in [#4359](https://github.com/prefix-dev/pixi/pull/4359)

#### New Contributors
* @chrisburr made their first contribution in [#4352](https://github.com/prefix-dev/pixi/pull/4352)
* @bnjbvr made their first contribution in [#4368](https://github.com/prefix-dev/pixi/pull/4368)

### [0.52.0] - 2025-08-14
#### ✨ Highlights

You can now use `pixi global` to install source dependencies.
```
pixi global install --path path/to/my-package my-package
```
At the moment, you still have to specify the package name, which we will improve on later!

#### ⚠️ Breaking Change

In `v0.51.0` we changed the environment variable overwriting logic.
This has be reverted in this release, as there are some issues with it.

#### Features

- Include named source dependencies through `pixi global` by @tdejager in [#4165](https://github.com/prefix-dev/pixi/pull/4165)

#### Documentation

- Fix example package name by @henningkayser in [#4331](https://github.com/prefix-dev/pixi/pull/4331)
- Add keyring auth support doc and bump setup-pixi action version by @olivier-lacroix in [#4332](https://github.com/prefix-dev/pixi/pull/4332)
- Pycharm integration via conda environments.txt file by @analog-cbarber in [#4290](https://github.com/prefix-dev/pixi/pull/4290)

#### Fixed

- Fish completion script by @ruben-arts in [#4315](https://github.com/prefix-dev/pixi/pull/4315)
- Update named arg schema by @bollwyvl in [#4324](https://github.com/prefix-dev/pixi/pull/4324)
- Revert environment logic changes by @Hofer-Julian in [#4346](https://github.com/prefix-dev/pixi/pull/4346)

#### Refactor

- Move all non cli code into `pixi_core` crate by @haecker-felix in [#4337](https://github.com/prefix-dev/pixi/pull/4337)

#### New Contributors
* @analog-cbarber made their first contribution in [#4290](https://github.com/prefix-dev/pixi/pull/4290)
* @haecker-felix made their first contribution in [#4337](https://github.com/prefix-dev/pixi/pull/4337)
* @henningkayser made their first contribution in [#4331](https://github.com/prefix-dev/pixi/pull/4331)

### [0.51.0] - 2025-08-12
#### ✨ Highlights

Pixi now supports `--skip` on install which means you can skip the installation of a package. 
Which can be useful for things like layering Docker images.

Pixi build got a lot of improvements, including the ability to use build backends from source.
Starting with this release you can get build backends from conda-forge. 
We will release stable versions of the build backends on conda-forge, and we maintain a rolling distribution on the `pixi-build-backends` channel. 
The documentation has been updated to reflect this change.

#### ⚠️ Breaking Change
The environment variable overwriting logic is changed. 
Previously, the variables in your own environment would overwrite the variables set in the Pixi manifest.
This is now reversed, meaning that the variables set in the Pixi manifest will overwrite the variables in your own environment.
More info can be found in the [documentation](https://pixi.sh/dev/reference/environment_variables/).

#### Added

- Add new configuration options for concurrency and experimental features by @zelosleone in [#4223](https://github.com/prefix-dev/pixi/pull/4223)
- Support for loong64 linux by @wszqkzqk in [#4163](https://github.com/prefix-dev/pixi/pull/4163)
- Add channels to the build-v1 apis by @baszalmstra in [#4249](https://github.com/prefix-dev/pixi/pull/4249)
- Support pip packages for `no-build-isolation` by @Hofer-Julian in [#4247](https://github.com/prefix-dev/pixi/pull/4247)
- Add better logging for build backends by @baszalmstra in [#4276](https://github.com/prefix-dev/pixi/pull/4276)
- Add `--build`, rename `--tool` to `--build_backends` by @lucascolley in [#4281](https://github.com/prefix-dev/pixi/pull/4281)
- Add support for PIXI_ENVIRONMENT_NAME and PS1 prompt modification by @zelosleone in [#4101](https://github.com/prefix-dev/pixi/pull/4101)
- Add the ability to skip install of local source dependencies by @olivier-lacroix in [#3092](https://github.com/prefix-dev/pixi/pull/3092)

#### Changed

- Prevent execution in parent of pixi home directory by @ytausch in [#4168](https://github.com/prefix-dev/pixi/pull/4168)
- Add `did you mean` suggestions for the cmds (built-in + pixi extensions) by @mrswastik-robot in [#4058](https://github.com/prefix-dev/pixi/pull/4058)
- Per platform extra options for pixi build configuration by @Hofer-Julian in [#4036](https://github.com/prefix-dev/pixi/pull/4036)
- Named args in `depends-on` by @lucascolley in [#4148](https://github.com/prefix-dev/pixi/pull/4148)
- Pass repodata records to build backend by @baszalmstra in [#4252](https://github.com/prefix-dev/pixi/pull/4252)
- Extract reporters out of pixi into its own crate by @tdejager in [#4266](https://github.com/prefix-dev/pixi/pull/4266)
- Allow using build backends from source by @baszalmstra in [#4145](https://github.com/prefix-dev/pixi/pull/4145)
- PyPI `requirements.txt` format by @lucascolley in [#4270](https://github.com/prefix-dev/pixi/pull/4270)
- Upgrade to uv 0.8.20 and get rid of non-async build dispatch calls by @tdejager in [#4289](https://github.com/prefix-dev/pixi/pull/4289)
- `SourceSpec` struct composed of `SourceLocationSpec` by @lucascolley in [#4305](https://github.com/prefix-dev/pixi/pull/4305)


#### Documentation

- Update documentation on task names by @photex in [#4230](https://github.com/prefix-dev/pixi/pull/4230)
- Update docs with `pixi-extensions` by @mrswastik-robot in [#4144](https://github.com/prefix-dev/pixi/pull/4144)
- Mention md-tui support for reading from stdin by @pavelzw in [#4268](https://github.com/prefix-dev/pixi/pull/4268)
- Add contributor docs for Python test snapshots by @lucascolley in [#4273](https://github.com/prefix-dev/pixi/pull/4273)
- Update documentation and related manifests by @ruben-arts in [#4279](https://github.com/prefix-dev/pixi/pull/4279)
- Fix 404 link by @pavelzw in [#4295](https://github.com/prefix-dev/pixi/pull/4295)
- Mention glow for viewing markdown in the terminal by @pavelzw in [#4288](https://github.com/prefix-dev/pixi/pull/4288)
- Fix typo by @pavelzw in [#4306](https://github.com/prefix-dev/pixi/pull/4306)[#4309](https://github.com/prefix-dev/pixi/pull/4309)
- Simplify and fix the `pixi build` getting started by @ruben-arts in [#4304](https://github.com/prefix-dev/pixi/pull/4304)
 

#### Fixed

- Improve testing speed by using prefix channel by @ruben-arts in [#4227](https://github.com/prefix-dev/pixi/pull/4227)
- Pixi-build preview-mode check by @remimimimimi in [#4224](https://github.com/prefix-dev/pixi/pull/4224)
- Relative path to package for pixi global by @wolfv in [#4200](https://github.com/prefix-dev/pixi/pull/4200)
- Replace syrupy with inline-snapshot by @lucascolley in [#4246](https://github.com/prefix-dev/pixi/pull/4246)
- Forward CTRL+C signal to `deno_task_shell` by @wolfv in [#4243](https://github.com/prefix-dev/pixi/pull/4243)
- Override environment variables based on priority by @magentaqin in [#3940](https://github.com/prefix-dev/pixi/pull/3940)
- Exclude env key 'PROJECT_ENV' and evaluate referenced variables by @magentaqin in [#4275](https://github.com/prefix-dev/pixi/pull/4275)
- Quick Demo example shell quoting by @notpeter in [#4285](https://github.com/prefix-dev/pixi/pull/4285)
- Feature activation environment variable priority by @magentaqin in [#4282](https://github.com/prefix-dev/pixi/pull/4282)
- Disable JLAP by default by @ruben-arts in [#4301](https://github.com/prefix-dev/pixi/pull/4301)


#### New Contributors
* @magentaqin made their first contribution in [#4282](https://github.com/prefix-dev/pixi/pull/4282)
* @notpeter made their first contribution in [#4285](https://github.com/prefix-dev/pixi/pull/4285)
* @lsetiawan made their first contribution in [#4248](https://github.com/prefix-dev/pixi/pull/4248)
* @Carbonhell made their first contribution in [#4263](https://github.com/prefix-dev/pixi/pull/4263)
* @matthewfeickert made their first contribution in [#4256](https://github.com/prefix-dev/pixi/pull/4256)
* @wszqkzqk made their first contribution in [#4163](https://github.com/prefix-dev/pixi/pull/4163)
* @photex made their first contribution in [#4230](https://github.com/prefix-dev/pixi/pull/4230)

### [0.50.2] - 2025-07-28
#### Documentation

- Update setup-pixi docs by @pavelzw in [#4207](https://github.com/prefix-dev/pixi/pull/4207)
- Update cli welcome in README by @pauljurczak in [#4211](https://github.com/prefix-dev/pixi/pull/4211)


#### Fixed

- Print build log if build fails by @Hofer-Julian in [#4205](https://github.com/prefix-dev/pixi/pull/4205)
- Increase retention of pixi artifacts by @Hofer-Julian in [#4215](https://github.com/prefix-dev/pixi/pull/4215)
- Network authentication pixi global by @ruben-arts in [#4222](https://github.com/prefix-dev/pixi/pull/4222)
- Netrc issue and hash mismatch by @baszalmstra in [#4218](https://github.com/prefix-dev/pixi/pull/4218)



#### New Contributors
* @pauljurczak made their first contribution in [#4211](https://github.com/prefix-dev/pixi/pull/4211)

### [0.50.1] - 2025-07-25
#### ✨ Highlights

Use `pixi import` to import `environment.yml` files into your Pixi manifest.

#### Added

- Add build profiles to not build in editable mode in `pixi build` by @baszalmstra in [#4202](https://github.com/prefix-dev/pixi/pull/4202)

#### Changed

- Implement `pixi import` for `environment.yml` by @lucascolley in [#4096](https://github.com/prefix-dev/pixi/pull/4096)

#### Fixed

- Global progress by @tdejager in [#4190](https://github.com/prefix-dev/pixi/pull/4190)
- Update rattler and add test for variable expansion by @Hofer-Julian in [#4199](https://github.com/prefix-dev/pixi/pull/4199)

### [0.50.0] - 2025-07-22
#### ✨ Highlights
This release contains loads of bug fixes and refactors, primarily to make `pixi build` more stable and feature rich in the near future.

#### Added

- Add `pypi-option.no-binary` by @thomas-maschler in [#4008](https://github.com/prefix-dev/pixi/pull/4008)
- Add explicit workspace inheritance syntax by @baszalmstra in [#4078](https://github.com/prefix-dev/pixi/pull/4078)
- Add `conda/outputs` and `conda/build_v2` backend protocol by @baszalmstra in [#4118](https://github.com/prefix-dev/pixi/pull/4118)
- Add cyclic dependency support by @baszalmstra in [#4143](https://github.com/prefix-dev/pixi/pull/4143)
- Rebuild source package if a build dependency changed by @baszalmstra in [#4171](https://github.com/prefix-dev/pixi/pull/4171)
- Add `pypi-options.dependency-overrides` to override pypi dependencies by @HernandoR  in [#3948](https://github.com/prefix-dev/pixi/pull/3948)

#### Changed

- Add `pixi init` as a suggestion in the error message, when `pyproject.toml` is without the `tool.pixi` section by @mrswastik-robot in [#3943](https://github.com/prefix-dev/pixi/pull/3943)
- Improve error messages when a python interpreter is needed by @tdejager in [#4075](https://github.com/prefix-dev/pixi/pull/4075)
- Manual validation of frozen and locked CLI arguments by @gshiba in [#4044](https://github.com/prefix-dev/pixi/pull/4044)
- Better error for unexpected packages from build backend by @baszalmstra in [#4098](https://github.com/prefix-dev/pixi/pull/4098)
- Implement stable hash for ProjectModelV1 to improve cache consistency by @baszalmstra in [#4094](https://github.com/prefix-dev/pixi/pull/4094)
- Upgrade to uv 0.7.20 by @tdejager in [#4091](https://github.com/prefix-dev/pixi/pull/4091)[#4115](https://github.com/prefix-dev/pixi/pull/4115)
- Use command dispatcher for `pixi global install` by @tdejager in [#4126](https://github.com/prefix-dev/pixi/pull/4126)
- Notify which conda packages may have influenced the conflict by @tdejager in [#4135](https://github.com/prefix-dev/pixi/pull/4135)
- Refactor spec implementation handling in global by @tdejager in [#4138](https://github.com/prefix-dev/pixi/pull/4138)
- Use command dispatcher for pixi build by @baszalmstra in [#4156](https://github.com/prefix-dev/pixi/pull/4156)

#### Documentation

- Add `site_description` by @lucascolley in [#4088](https://github.com/prefix-dev/pixi/pull/4088)
- Improve the `system-requirements` documentation by @ruben-arts in [#4068](https://github.com/prefix-dev/pixi/pull/4068)
- Enable `content.code.select` by @lucascolley in [#4092](https://github.com/prefix-dev/pixi/pull/4092)
- Add `conda-deny` documentation by @PaulKMueller in [#4090](https://github.com/prefix-dev/pixi/pull/4090) [#4124](https://github.com/prefix-dev/pixi/pull/4124)
- Update `setup-pixi` docs for pixi-url-bearer-token by @ytausch in [#4127](https://github.com/prefix-dev/pixi/pull/4127)
- Update the python tutorial to use the workspace command by @rongou in [#4128](https://github.com/prefix-dev/pixi/pull/4128)
- Update `setup-pixi` docs for 0.8.13 by @ytausch in [#4175](https://github.com/prefix-dev/pixi/pull/4175)
- Add `geovista` to community.md by @bjlittle in [#4183](https://github.com/prefix-dev/pixi/pull/4183)

#### Fixed

- Only print release notes on new version with `self-update` by @lucascolley in [#4054](https://github.com/prefix-dev/pixi/pull/4054)
- Add an early check, before creating directories for `<non-existent-env>` while uninstalling them by @mrswastik-robot in [#4049](https://github.com/prefix-dev/pixi/pull/4049)
- Update template variable for extra index URLs in init file by @noamgot in [#4072](https://github.com/prefix-dev/pixi/pull/4072)
- Allow to set `pypi-config.allow-insecure-host` by @zen-xu in [#4107](https://github.com/prefix-dev/pixi/pull/4107)

#### New Contributors
* @rongou made their first contribution in [#4128](https://github.com/prefix-dev/pixi/pull/4128)
* @PaulKMueller made their first contribution in [#4124](https://github.com/prefix-dev/pixi/pull/4124)
* @gshiba made their first contribution in [#4044](https://github.com/prefix-dev/pixi/pull/4044)
* @thomas-maschler made their first contribution in [#4008](https://github.com/prefix-dev/pixi/pull/4008)
* @bjlittle made their first contribution in [#4183](https://github.com/prefix-dev/pixi/pull/4183)

### [0.49.0] - 2025-06-30
#### ✨ Highlights
This release enables `pixi` to pick up extensions that are installed as `pixi-`.
This is similar to `cargo`, `git` and other tools.
This means that you can now install extensions like this:

```shell
pixi global install pixi-pack
pixi pack
```

It also allows you to use `pixi exec` more easily:
```shell
pixi exec --with numpy python -c "import numpy; print(numpy.__version__)"
# Previous command is equivalent to:
pixi exec --spec numpy --spec python python -c "import numpy; print(numpy.__version__)"
```

#### Added

- Add `turtlebot4` simulation example to `ros2-nav2` by @wep21 in [#3988](https://github.com/prefix-dev/pixi/pull/3988)
- Add `--with` option to `pixi exec` by @lucascolley in [#4011](https://github.com/prefix-dev/pixi/pull/4011)
- Implement external `pixi-` command discovery for pixi extensions by @mrswastik-robot in [#3968](https://github.com/prefix-dev/pixi/pull/3968)


#### Changed

- Remove egg-info from gitignore and fix whitespace in beginning by @pavelzw in [#3964](https://github.com/prefix-dev/pixi/pull/3964)

#### Documentation

- Refactor getting started and python tutorial by @ruben-arts in [#3977](https://github.com/prefix-dev/pixi/pull/3977)
- Add links to backend docs by @Hofer-Julian in [#4010](https://github.com/prefix-dev/pixi/pull/4010)
- Improve content and layout by @ruben-arts in [#4003](https://github.com/prefix-dev/pixi/pull/4003)
- Update pixi-pack documentation for parallel downloads by @delsner in [#4018](https://github.com/prefix-dev/pixi/pull/4018)
- Add pixi pack to our extensions by @Hofer-Julian in [#4019](https://github.com/prefix-dev/pixi/pull/4019)
- Explain conda and pypi mix by @ruben-arts in [#4022](https://github.com/prefix-dev/pixi/pull/4022)
- Mention other extensions by @pavelzw in [#4026](https://github.com/prefix-dev/pixi/pull/4026)
- Update pixi-pack docs for separate packages by @delsner in [#4025](https://github.com/prefix-dev/pixi/pull/4025)
- Update title pixi-diff-to-markdown by @Hofer-Julian in [#4027](https://github.com/prefix-dev/pixi/pull/4027)
- Add example of passing `arg` to `depended-on` task by @theavey in [#4030](https://github.com/prefix-dev/pixi/pull/4030)
- Tweak nav headings by @lucascolley in [#4045](https://github.com/prefix-dev/pixi/pull/4045)
- Fix formatting in pixi-pack by @pavelzw in [#4050](https://github.com/prefix-dev/pixi/pull/4050)

#### Fixed

- Multi output handling in Pixi by @Hofer-Julian in [#3961](https://github.com/prefix-dev/pixi/pull/3961)
- Check for the environments not the environments dir by @ruben-arts in [#4005](https://github.com/prefix-dev/pixi/pull/4005)
- Only trigger rebuild when relevant parts of the package manifest changed by @Hofer-Julian in [#3966](https://github.com/prefix-dev/pixi/pull/3966)
- Lazy raise error of pypi building environment by @gzm55 in [#4009](https://github.com/prefix-dev/pixi/pull/4009)
- Don't error on readonly fs with ignore files by @ruben-arts in [#3984](https://github.com/prefix-dev/pixi/pull/3984)
- Fix example & tweak some wording by @lucascolley in [#4046](https://github.com/prefix-dev/pixi/pull/4046)

#### Refactor

- Building with command dispatcher by @baszalmstra in [#3967](https://github.com/prefix-dev/pixi/pull/3967)


#### New Contributors
* @theavey made their first contribution in [#4030](https://github.com/prefix-dev/pixi/pull/4030)
* @xhochy made their first contribution in [#4014](https://github.com/prefix-dev/pixi/pull/4014)
* @wep21 made their first contribution in [#3988](https://github.com/prefix-dev/pixi/pull/3988)

### [0.48.2] - 2025-06-16
#### ✨ Highlights

This is a minor release with a couple of bugfixes.
A new feature is the support for `mojoproject.toml` files which is used to develop on projects written in the Mojo programming language.
This enables users to migrate from the [deprecated](https://docs.modular.com/magic/) Magic package manager to Pixi.


#### Added

- Support legacy mojoproject.toml by @zbowling in [#3942](https://github.com/prefix-dev/pixi/pull/3942)


#### Changed

- Update pixi-install-to-prefix by @ytausch in [#3924](https://github.com/prefix-dev/pixi/pull/3924)
- Negotiate pixi build RPC interface through interface package by @Hofer-Julian in [#3927](https://github.com/prefix-dev/pixi/pull/3927)


#### Documentation

- Fix `discover_pixi` docstring by @Hofer-Julian in [#3928](https://github.com/prefix-dev/pixi/pull/3928)


#### Fixed

- Fix caching by @Hofer-Julian in [#3933](https://github.com/prefix-dev/pixi/pull/3933)
- Always pass index locations by @nichmor in [#3947](https://github.com/prefix-dev/pixi/pull/3947)
- Prefer to use MatchSpec instead of wildcard as package filter by @trim21 in [#3926](https://github.com/prefix-dev/pixi/pull/3926)
- Typo in the doc strings by @Hofer-Julian in [#3954](https://github.com/prefix-dev/pixi/pull/3954)


#### Refactor

- Conda solving with command dispatcher by @baszalmstra in [#3909](https://github.com/prefix-dev/pixi/pull/3909)


#### Removed

- Remove `error_to_snapshot` by @Hofer-Julian in [#3934](https://github.com/prefix-dev/pixi/pull/3934)



### [0.48.1] - 2025-06-10
#### ✨ Highlights

This is a minor release with a couple of bugs fixed.
Additionally, `pixi self-update` accepts now the flags `--force` and `--no-release-note`.


#### Added

- Add cli options for self-update: --force and --no-release-note by @gzm55 in [#3888](https://github.com/prefix-dev/pixi/pull/3888)
- Add pixi build testsuite by @Hofer-Julian in [#3891](https://github.com/prefix-dev/pixi/pull/3891)


#### Fixed

- Discovery error message by @Hofer-Julian in [#3903](https://github.com/prefix-dev/pixi/pull/3903)
- `pixi lock` reporting by @Hofer-Julian in [#3896](https://github.com/prefix-dev/pixi/pull/3896)
- Backslashes in editable path by @tdejager in [#3895](https://github.com/prefix-dev/pixi/pull/3895)
- No longer panics, when a conda dependency is a PyPI get dependency by @ruben-arts in [#3905](https://github.com/prefix-dev/pixi/pull/3905)


#### Removed

- Remove pixi build tests by @Hofer-Julian in [#3892](https://github.com/prefix-dev/pixi/pull/3892)


#### New Contributors
* @simonjung1603 made their first contribution in [#3565](https://github.com/prefix-dev/pixi/pull/3565)

### [0.48.0] - 2025-06-02
#### ✨ Highlights

Support for recursive source run dependencies when using `pixi build`.
This means, you can now add source dependencies in the `run-dependencies` section of your Pixi package:

```toml
[package.run-dependencies]
cpp_math = { path = "packages/cpp_math" }
```

#### Added

- Add `XDG_CONFIG_HOME` as configuration location on macOS by @ruben-arts in [#3759](https://github.com/prefix-dev/pixi/pull/3759)
- Support relative path input globs for `pixi build` by @nichmor in [#3812](https://github.com/prefix-dev/pixi/pull/3812)
- Add `condapackageignore` file to exclude `.pixi` directory from builds by @zelosleone in [#3840](https://github.com/prefix-dev/pixi/pull/3840)

#### Changed

- Improve type of outputs, add `Eq`, `PartialEq`, etc. by @wolfv in [#3822](https://github.com/prefix-dev/pixi/pull/3822)
- Transform reporter events into tree by @baszalmstra in [#3834](https://github.com/prefix-dev/pixi/pull/3834)
- Add release notes to the `self-update` including `--dry-run` by @chrisliebaer in [#3397](https://github.com/prefix-dev/pixi/pull/3397)
- Migrate to `uv_distribution_types` for package requirements and update uv by @zelosleone in [#3872](https://github.com/prefix-dev/pixi/pull/3872)

#### Documentation

- Start using recursive source run dependencies by @Hofer-Julian in [#3768](https://github.com/prefix-dev/pixi/pull/3768)
- Simplify documentation frontpage by @ruben-arts in [#3802](https://github.com/prefix-dev/pixi/pull/3802)
- Add security policy by @pavelzw in [#3823](https://github.com/prefix-dev/pixi/pull/3823)
- Fix typo in multi_environment.md by @AH-Merii in [#3797](https://github.com/prefix-dev/pixi/pull/3797)
- Update pixi-pack docs for allow `--inject`ing wheels by @e8035669 in [#3853](https://github.com/prefix-dev/pixi/pull/3853)
- Separator backend override by @Hofer-Julian in [#3857](https://github.com/prefix-dev/pixi/pull/3857)

#### Fixed

- Adapt for backend update by @Hofer-Julian in [#3767](https://github.com/prefix-dev/pixi/pull/3767)
- Hashing for same package-name by @tdejager in [#3775](https://github.com/prefix-dev/pixi/pull/3775)
- Recursive source run deps by @Hofer-Julian in [#3712](https://github.com/prefix-dev/pixi/pull/3712)
- Workspace version and name inheritance by @baszalmstra in [#3786](https://github.com/prefix-dev/pixi/pull/3786)
- Examples in self-update_extender by @sjpfenninger in [#3793](https://github.com/prefix-dev/pixi/pull/3793)
- Take into account tasks arguments when caching by @nichmor in [#3782](https://github.com/prefix-dev/pixi/pull/3782)
- Hanging on ssh passphrase by @nichmor in [#3761](https://github.com/prefix-dev/pixi/pull/3761)
- Keep index on pypi dependency upgrade by @remimimimimi in [#3746](https://github.com/prefix-dev/pixi/pull/3746)
- Help user when command not found by @ruben-arts in [#3803](https://github.com/prefix-dev/pixi/pull/3803)
- Failing ci and improve test by @tdejager in [#3798](https://github.com/prefix-dev/pixi/pull/3798)
- Space is a valid separator between date and time in `exclude-newer` by @trim21 in [#3764](https://github.com/prefix-dev/pixi/pull/3764)
- Source globs and make them unique by @wolfv in [#3831](https://github.com/prefix-dev/pixi/pull/3831)
- Validate `depends-on` must be a list by @YoganshSharma in [#3832](https://github.com/prefix-dev/pixi/pull/3832)
- Path diff not calculated on Windows by @Hofer-Julian in [#3824](https://github.com/prefix-dev/pixi/pull/3824)
- Unify reqwest Client for self_update when downloading archives by @gzm55 in [#3346](https://github.com/prefix-dev/pixi/pull/3346)
- Fix main CI with some clippy and conflict fixes by @ruben-arts in [#3858](https://github.com/prefix-dev/pixi/pull/3858)
- Filter duplicated path_diff entries for pixi global by @Hofer-Julian in [#3859](https://github.com/prefix-dev/pixi/pull/3859)
- Update help message for unsupported PyPI platform error to include adding Python dependency by @zelosleone in [#3861](https://github.com/prefix-dev/pixi/pull/3861)

#### Refactor

- Command dispatcher by @baszalmstra in [#3791](https://github.com/prefix-dev/pixi/pull/3791)
- Move `pixi_docs` to Cargo workspace by @Hofer-Julian in [#3844](https://github.com/prefix-dev/pixi/pull/3844)
- Move fetching of source metadata to command dispatcher by @baszalmstra in [#3843](https://github.com/prefix-dev/pixi/pull/3843)
- Alphabetize command list in CLI help by @dhirschfeld in [#3817](https://github.com/prefix-dev/pixi/pull/3817)

#### New Contributors
* @e8035669 made their first contribution in [#3853](https://github.com/prefix-dev/pixi/pull/3853)
* @chrisliebaer made their first contribution in [#3397](https://github.com/prefix-dev/pixi/pull/3397)
* @YoganshSharma made their first contribution in [#3832](https://github.com/prefix-dev/pixi/pull/3832)
* @remimimimimi made their first contribution in [#3746](https://github.com/prefix-dev/pixi/pull/3746)
* @sjpfenninger made their first contribution in [#3793](https://github.com/prefix-dev/pixi/pull/3793)

### [0.47.0] - 2025-05-12
#### ✨ Highlights

- We now support `exclude-newer` to avoid getting packages that are build after the given date/timestamp.
- The minijinja syntax now also works in `depends-on` and `inputs`/`outputs` of the tasks.

#### Added

- Add `--check` option to lock command by @noamgot in [#3663](https://github.com/prefix-dev/pixi/pull/3663)
- Add `exclude-newer` to workspace by @baszalmstra in [#3633](https://github.com/prefix-dev/pixi/pull/3633)
- Add environment size and prefix to pixi info by @MridulS in [#3674](https://github.com/prefix-dev/pixi/pull/3674)
- Support jinja inputs outputs by @prsabahrami in [#3638](https://github.com/prefix-dev/pixi/pull/3638)
- Support minijinja templates in arguments passed to `depends-on` by @prsabahrami in [#3668](https://github.com/prefix-dev/pixi/pull/3668)

#### Changed

- Change the way paths are handled by @tdejager in [#3658](https://github.com/prefix-dev/pixi/pull/3658)
- Add build section to pixi config by @pavelzw in [#3502](https://github.com/prefix-dev/pixi/pull/3502)
- Enable `tool.uv.sources` to work by @tdejager in [#3636](https://github.com/prefix-dev/pixi/pull/3636)
- Installer use PIXI_REPOURL env as download base by @gzm55 in [#3558](https://github.com/prefix-dev/pixi/pull/3558)
- Use a table for task list by @bollwyvl in [#3708](https://github.com/prefix-dev/pixi/pull/3708)

#### Fixed

- Add support for Visual Studio 2022 in dependencies and lock file by @zelosleone in [#3643](https://github.com/prefix-dev/pixi/pull/3643)
- Json prints of the tasks by @prsabahrami in [#3637](https://github.com/prefix-dev/pixi/pull/3637)
- Allow no build isolation for all packages by @baszalmstra in [#3657](https://github.com/prefix-dev/pixi/pull/3657)
- Using the value of pattern_timeout for the shell initialization warning message by @noamgot in [#3660](https://github.com/prefix-dev/pixi/pull/3660)
- Pypi panic with path dependencies in lock file by @Hofer-Julian in [#3690](https://github.com/prefix-dev/pixi/pull/3690)
- Don't remove non-owned PyPI installs by @tdejager in [#3694](https://github.com/prefix-dev/pixi/pull/3694)
- Task arg schema by @Hofer-Julian in [#3703](https://github.com/prefix-dev/pixi/pull/3703)
- Normalize URL by removing SHA256 fragment in requirement source by @zelosleone in [#3696](https://github.com/prefix-dev/pixi/pull/3696)
- Error on improper pyproject.toml configuration by @ruben-arts in [#3704](https://github.com/prefix-dev/pixi/pull/3704)
- Package target selector platform serialization by @ruben-arts in [#3720](https://github.com/prefix-dev/pixi/pull/3720)
- Task arg parsing panic by @Hofer-Julian in [#3731](https://github.com/prefix-dev/pixi/pull/3731)
- Remove priority-queue spec limit due to rust edition by @gzm55 in [#3672](https://github.com/prefix-dev/pixi/pull/3672)

#### Refactor

- Move pypi requirement to crate by @baszalmstra in [#3689](https://github.com/prefix-dev/pixi/pull/3689)

#### Documentation

- Fix fish completion command by @pavelzw in [#3642](https://github.com/prefix-dev/pixi/pull/3642)
- Download from github release by @Hofer-Julian in [#3653](https://github.com/prefix-dev/pixi/pull/3653)
- Mention pixi docker blog post by @pavelzw in [#3655](https://github.com/prefix-dev/pixi/pull/3655)
- Remove duplicate entry of ndonnx by @gnodar01 in [#3664](https://github.com/prefix-dev/pixi/pull/3664)
- Clarify default env by @Hofer-Julian in [#3661](https://github.com/prefix-dev/pixi/pull/3661)
- Fix typo by @mdeff in [#3673](https://github.com/prefix-dev/pixi/pull/3673)
- Clarify which pyproject_toml is used by @zoed191 in [#3681](https://github.com/prefix-dev/pixi/pull/3681)
- Add s3 middleware and mirror middleware to pixi-pack by @pavelzw in [#3679](https://github.com/prefix-dev/pixi/pull/3679)
- Add pyfixest to community projects by @s3alfisc in [#3693](https://github.com/prefix-dev/pixi/pull/3693)
- Update starship page by @Hofer-Julian in [#3697](https://github.com/prefix-dev/pixi/pull/3697)
- Fix broken project manifest link by @lkstrp in [#3713](https://github.com/prefix-dev/pixi/pull/3713)
- Mention YouTrack tracking issues for pixi by @pavelzw in [#3748](https://github.com/prefix-dev/pixi/pull/3748)
- Support local `pixi-pack` executables for self-extracting by @FSP1020 in [#3745](https://github.com/prefix-dev/pixi/pull/3745)
- Mention `pixi-install-to-prefix` by @pavelzw in [#3750](https://github.com/prefix-dev/pixi/pull/3750)
- Add install method `scoop` on Windows by @trim21 in [#3737](https://github.com/prefix-dev/pixi/pull/3737)
- Add a local repo as the dependency by @kemingy in [#3666](https://github.com/prefix-dev/pixi/pull/3666)



#### New Contributors
* @FSP1020 made their first contribution in [#3745](https://github.com/prefix-dev/pixi/pull/3745)
* @lkstrp made their first contribution in [#3713](https://github.com/prefix-dev/pixi/pull/3713)
* @zelosleone made their first contribution in [#3696](https://github.com/prefix-dev/pixi/pull/3696)
* @s3alfisc made their first contribution in [#3693](https://github.com/prefix-dev/pixi/pull/3693)
* @zoed191 made their first contribution in [#3681](https://github.com/prefix-dev/pixi/pull/3681)
* @MridulS made their first contribution in [#3674](https://github.com/prefix-dev/pixi/pull/3674)
* @mdeff made their first contribution in [#3673](https://github.com/prefix-dev/pixi/pull/3673)
* @gnodar01 made their first contribution in [#3664](https://github.com/prefix-dev/pixi/pull/3664)

### [0.46.0] - 2025-04-22
#### ⚠️ Breaking Change

`arg` names in `tasks` can no longer contain dashes (`-`).
This restriction is due to the integration of `Minijinja` for rendering tasks, where dashes could be misinterpreted as a subtraction operator.

#### ✨ Highlights
This release comes with another set of features for the `tasks`!
-  The command of a task is now able to use `minijinja` for rendering the command.
```toml
 [tasks]
# The arg `text`, converted to uppercase, will be printed.
task1 = { cmd = "echo {{ text | upper }}", args = ["text"] }
# If arg `text` contains 'hoi', it will be converted to lowercase. The result will be printed.
task2 = { cmd = "echo {{ text | lower if 'hoi' in text }}", args = [
  { arg = "text", default = "" },
] }
# With `a` and `b` being strings, they will be appended and then printed.
task3 = { cmd = "echo {{ a + b }}", args = ["a", { arg = "b", default = "!" }] }
# `names` will be split by whitespace and then every name will be printed separately
task4 = { cmd = "{% for name in names | split %} echo {{ name }};{% endfor %}", args = [
  "names",
] }
```
- Shortened composition of tasks with `depends-on` key.
```toml
[tasks]
test-all = [{ task = "test", args = ["all"] }]
# Equivalent to: test-all = { depends-on = [{task = "test", args = ["all"] }]}
```
-  The `depends-on` key can now include the environment that the task should run in.
```toml
[tasks]
# Using the shortened composition of tasks
test-all = [
  { task = "test", environment = "py311" },
  { task = "test", environment = "py312" },
]
```

#### Added

- Integrate minijinja for tasks' command's rendering by @prsabahrami in [#3579](https://github.com/prefix-dev/pixi/pull/3579)
- Support for riscv64 linux by @Hofer-Julian in [#3606](https://github.com/prefix-dev/pixi/pull/3606)
- Task Environment Selection by @prsabahrami in [#3501](https://github.com/prefix-dev/pixi/pull/3501)
- Shortened task composition with `depends-on` key by @prsabahrami in [#3450](https://github.com/prefix-dev/pixi/pull/3540)


#### Changed

- Format shell script with shfmt by @gzm55 in [#3552](https://github.com/prefix-dev/pixi/pull/3552)
- Improve error message at missing pixi section by @joyanedel in [#3516](https://github.com/prefix-dev/pixi/pull/3516)
- Install.sh supports installing without tar and unzip commands by @gzm55 in [#3551](https://github.com/prefix-dev/pixi/pull/3551)


#### Documentation

- Community: add `xsf` by @lucascolley in [#3515](https://github.com/prefix-dev/pixi/pull/3515)
- Mention installation with `wget` instead of `curl` by @paugier in [#3547](https://github.com/prefix-dev/pixi/pull/3547)
- Fix typo in `advanced_tasks.md` by @AH-Merii in [#3555](https://github.com/prefix-dev/pixi/pull/3555)
- Update to call pixi task "start" in Index.md by @philipreese in [#3487](https://github.com/prefix-dev/pixi/pull/3487)
- Migrate ros2.md example to use robostack-humble channel by @traversaro in [#3520](https://github.com/prefix-dev/pixi/pull/3520)
- Provide guidance on using `direnv` by @phreed in [#3513](https://github.com/prefix-dev/pixi/pull/3513)
- Remove unused ordered list in system_requirements.md by @kemingy in [#3590](https://github.com/prefix-dev/pixi/pull/3590)
- Update `authentication.md` to fix typo in package name by @PanTheDev in [#3615](https://github.com/prefix-dev/pixi/pull/3615)
- Replace `project` with `workspace` by @munechika-koyo in [#3623](https://github.com/prefix-dev/pixi/pull/3623) and in [#3620](https://github.com/prefix-dev/pixi/pull/3620)
- Rename `pixi build` examples by @Hofer-Julian in [#3632](https://github.com/prefix-dev/pixi/pull/3632)


#### Fixed

- Updating windows `installation.md` to correspond to `installation.ps1` by @Ahschreyer in [#3507](https://github.com/prefix-dev/pixi/pull/3507)
- Fix panic for `pixi task list` for platform specific tasks by @synapticarbors in [#3510](https://github.com/prefix-dev/pixi/pull/3510)
- Improve error message if build backend crashes by @baszalmstra in [#3543](https://github.com/prefix-dev/pixi/pull/3543)
- Check unzip command on msys and add ITs for install.sh by @gzm55 in [#3458](https://github.com/prefix-dev/pixi/pull/3458)
- Shell-hook `--no-completion` by @Hofer-Julian in [#3553](https://github.com/prefix-dev/pixi/pull/3553)
- Fixed a typo in init.rs by @noamgot in [#3561](https://github.com/prefix-dev/pixi/pull/3561)
- Pixi-shell: bump timeout to 3 secs, fix docs link by @wolfv in [#3528](https://github.com/prefix-dev/pixi/pull/3528)
- Invalidate lock-file if a pypi dependency is requested by @baszalmstra in [#3562](https://github.com/prefix-dev/pixi/pull/3562)
- Build backend missing executable by @baszalmstra in [#3527](https://github.com/prefix-dev/pixi/pull/3527)
- Community page link in README by @elonzh in [#3595](https://github.com/prefix-dev/pixi/pull/3595)
- Mark `add_tests::add_pypi_git` as an online test by @mgorny in [#3586](https://github.com/prefix-dev/pixi/pull/3586)
- Update `git-cliff` by @Hofer-Julian in [#3614](https://github.com/prefix-dev/pixi/pull/3614)
- Pypi local directory satisfiability by @tdejager in [#3631](https://github.com/prefix-dev/pixi/pull/3631)
- Don't keep reinstalling local pypi archives by @tdejager in [#3618](https://github.com/prefix-dev/pixi/pull/3618)
- Don't resolve for previously locked platforms by @baszalmstra in [#3635](https://github.com/prefix-dev/pixi/pull/3635)


#### Refactor

- Reduce difference between default and named feature by @baszalmstra in [#3545](https://github.com/prefix-dev/pixi/pull/3545)
- Update `pixi.toml` to use args by @prsabahrami in [#3531](https://github.com/prefix-dev/pixi/pull/3531)
- Refactor of pypi installer type by @tdejager in [#3563](https://github.com/prefix-dev/pixi/pull/3563)


#### New Contributors
* @renovate[bot] made their first contribution in [#3626](https://github.com/prefix-dev/pixi/pull/3626)
* @munechika-koyo made their first contribution in [#3623](https://github.com/prefix-dev/pixi/pull/3623)
* @elonzh made their first contribution in [#3595](https://github.com/prefix-dev/pixi/pull/3595)
* @Ahschreyer made their first contribution in [#3507](https://github.com/prefix-dev/pixi/pull/3507)
* @kemingy made their first contribution in [#3590](https://github.com/prefix-dev/pixi/pull/3590)
* @joyanedel made their first contribution in [#3516](https://github.com/prefix-dev/pixi/pull/3516)
* @phreed made their first contribution in [#3513](https://github.com/prefix-dev/pixi/pull/3513)
* @AH-Merii made their first contribution in [#3555](https://github.com/prefix-dev/pixi/pull/3555)
* @paugier made their first contribution in [#3547](https://github.com/prefix-dev/pixi/pull/3547)
* @mwiebe made their first contribution in [#3519](https://github.com/prefix-dev/pixi/pull/3519)


### [0.45.0] - 2025-04-07
#### ✨ Highlights

This release brings in numerous improvements and bug fixes and one big feature: argument variables tasks!
If you do add `args`, you will have more convenient way of specifying arguments, which works with pipes and even allows you to set defaults.

Let's say you define this manifest:

```toml
[tasks.install]
cmd = "cargo install {{ type }} --path {{ path }}"
args = ["path", { arg = "type", default = "--release" }] # `path` is mandatory, `type` is optional with a default
```

Both of the invocations now work, since `type` is optional:

```bash
pixi run install /path/to/manifest
pixi run install /path/to/manifest --debug
```

If you don't specify `args` for your tasks everything which you append to the CLI will also be appended to the task.

```toml
[tasks.install]
cmd = "cargo install"
```

Therefore, running `pixi run install --debug --path /path/to/manifest` will lead to `cargo install --debug --path /path/to/manifest` being run inside the environment.
This was already the behavior before this release, so existing tasks should continue to work.

Learn more in our documentation: https://pixi.sh/v0.45.0/workspace/advanced_tasks/#using-task-arguments

#### Changed

- Argument variables in tasks by @prsabahrami in [#3433](https://github.com/prefix-dev/pixi/pull/3433)
- Make workspace name optional by @baszalmstra in [#3526](https://github.com/prefix-dev/pixi/pull/3526)


#### Documentation

- Added task cwd default behaviour by @danpal96 in [#3470](https://github.com/prefix-dev/pixi/pull/3470)
- Move direnv section to separate page by @pavelzw in [#3472](https://github.com/prefix-dev/pixi/pull/3472)
- Exclude extender files from search by @Hofer-Julian in [#3473](https://github.com/prefix-dev/pixi/pull/3473)
- Update getting_started.md to correctly reference py313 instead of py312 by @philipreese in [#3489](https://github.com/prefix-dev/pixi/pull/3489)
- Move environment var section by @Hofer-Julian in [#3498](https://github.com/prefix-dev/pixi/pull/3498)
- Rename remaining `pixi project` to `pixi workspace` by @Hofer-Julian in [#3486](https://github.com/prefix-dev/pixi/pull/3486)
- Mention pypi support in pixi-pack by @pavelzw in [#3508](https://github.com/prefix-dev/pixi/pull/3508)
- Document that activation scripts are not simply sourced by @traversaro in [#3506](https://github.com/prefix-dev/pixi/pull/3506)
- Update pixi-pack docs for ignoring non-wheel pypi packages by @delsner in [#3523](https://github.com/prefix-dev/pixi/pull/3523)


#### Fixed

- `pixi run deno` by @Hofer-Julian in [#3484](https://github.com/prefix-dev/pixi/pull/3484)
- 'pixi config list proxy-config' by @gzm55 in [#3497](https://github.com/prefix-dev/pixi/pull/3497)
- Shell-hook, avoid running unexpected commands by @gzm55 in [#3493](https://github.com/prefix-dev/pixi/pull/3493)
- `pixi global` stop checking for `quicklaunch` on Windows by @Hofer-Julian in [#3521](https://github.com/prefix-dev/pixi/pull/3521)


#### Performance

- Only call `pixi global` completion functions when necessary by @Hofer-Julian in [#3477](https://github.com/prefix-dev/pixi/pull/3477)

#### Refactor

- Change section header from [project] to [workspace] in the docs source files by @prsabahrami in [#3494](https://github.com/prefix-dev/pixi/pull/3494)


#### New Contributors
* @philipreese made their first contribution in [#3489](https://github.com/prefix-dev/pixi/pull/3489)
* @danpal96 made their first contribution in [#3470](https://github.com/prefix-dev/pixi/pull/3470)

### [0.44.0] - 2025-03-31
#### ✨ Highlights

- ⌨️ Support of [shell completions](https://pixi.sh/v0.44.0/global_tools/introduction/#shell-completions) with `pixi global`
- 🛠️ [`requires-pixi`](https://pixi.sh/v0.44.0/reference/pixi_manifest/#requires-pixi-optional) key in `pixi.toml. This is especially useful to require a minimum Pixi version when using the manifest.
- ⚙️ Add configuration to run [post link scripts](https://pixi.sh/v0.44.0/reference/pixi_configuration/#run-post-link-scripts). This is useful for workspaces with packages which fail to work otherwise. We advise to only enable the config if it is actually needed.



#### Added

- Add run export information to search by @baszalmstra in [#3428](https://github.com/prefix-dev/pixi/pull/3428)
- Add 'requires-pixi' manifest key and subcommands to control the eldest pixi for building a project by @gzm55 in [#3358](https://github.com/prefix-dev/pixi/pull/3358)
- Add proxy-config and activate https/http proxy for pixi commands by @gzm55 in [#3320](https://github.com/prefix-dev/pixi/pull/3320)
- Support shell completions with `pixi global` by @wolfv in [#3319](https://github.com/prefix-dev/pixi/pull/3319)
- Add configuration to run link scripts by @ruben-arts in [#3347](https://github.com/prefix-dev/pixi/pull/3347)

#### Changed

- Make install.sh posix compliant by @gzm55 in [#3450](https://github.com/prefix-dev/pixi/pull/3450)

#### Documentation

- Remove reference to archived mybinder.org example by @scottyhq in [#3435](https://github.com/prefix-dev/pixi/pull/3435)
- Tweak config naming convention note by @salim-b in [#3444](https://github.com/prefix-dev/pixi/pull/3444)
- Mention self-extracting binaries in pixi_pack by @gdementen in [#3446](https://github.com/prefix-dev/pixi/pull/3446)
- Remove main heading by @Hofer-Julian in [#3454](https://github.com/prefix-dev/pixi/pull/3454)
- Community: add metrology-apis by @lucascolley in [#3461](https://github.com/prefix-dev/pixi/pull/3461)

#### Fixed

- Error on package not provided by build backend by @baszalmstra in [#3430](https://github.com/prefix-dev/pixi/pull/3430)
- Git does not update source files by @nichmor in [#3425](https://github.com/prefix-dev/pixi/pull/3425)
- `pixi global` make sure that `prefix_path_entries` are always in `path_diff` by @Hofer-Julian in [#3407](https://github.com/prefix-dev/pixi/pull/3407)

#### Performance

- Skip redundant installation in `pixi global update` by @Glatzel in [#3404](https://github.com/prefix-dev/pixi/pull/3404)


#### New Contributors
* @gdementen made their first contribution in [#3446](https://github.com/prefix-dev/pixi/pull/3446)
* @scottyhq made their first contribution in [#3435](https://github.com/prefix-dev/pixi/pull/3435)

### [0.43.3] - 2025-03-25


#### Added

- Add build folder for pixi build by @nichmor in [#3410](https://github.com/prefix-dev/pixi/pull/3410)


#### Fixed

- Discover closest package when running pixi build by @nichmor in [#3412](https://github.com/prefix-dev/pixi/pull/3412)
- Propagate error diagnostics from backends by @baszalmstra in [#3426](https://github.com/prefix-dev/pixi/pull/3426)



### [0.43.2] - 2025-03-25
#### ✨ Highlights

- Various `pixi global` fixes: [#3420](https://github.com/prefix-dev/pixi/pull/3420), [#3420](https://github.com/prefix-dev/pixi/pull/3420)
- Start a non-login shell using `pixi shell`: [#3419](https://github.com/prefix-dev/pixi/pull/3419)

#### Documentation

- Don't mirror conda-forge's label channels by @pavelzw in [#3409](https://github.com/prefix-dev/pixi/pull/3409)

#### Fixed

- Broken `pixi global` migration by @Hofer-Julian in [#3420](https://github.com/prefix-dev/pixi/pull/3420)
- Ignore broken shortcuts by @Hofer-Julian in [#3422](https://github.com/prefix-dev/pixi/pull/3422)
- Start a non-login shell using `pixi shell` by @Ehab-Ibrahim in [#3419](https://github.com/prefix-dev/pixi/pull/3419)


#### New Contributors
* @Ehab-Ibrahim made their first contribution in [#3419](https://github.com/prefix-dev/pixi/pull/3419)

### [0.43.1] - 2025-03-24
#### ✨ Highlights

- Fixes problems introduced with `0.43.0`
- Adds support for uv mirror middleware
- Makes `pixi shell` more robust.


#### Changed

- Upgrade uv to 0.6.9 by @gzm55 in [#3374](https://github.com/prefix-dev/pixi/pull/3374)
- Global path diff by @Hofer-Julian in [#3384](https://github.com/prefix-dev/pixi/pull/3384)
- Add uv mirror middlewares by @gzm55 in [#3306](https://github.com/prefix-dev/pixi/pull/3306)


#### Documentation

- Fix broken link in changelog by @Hofer-Julian in [#3391](https://github.com/prefix-dev/pixi/pull/3391)
- Fix a typo in set_extender.md by @trim21 in [#3386](https://github.com/prefix-dev/pixi/pull/3386)
- Fix redirect by @Hofer-Julian in [#3396](https://github.com/prefix-dev/pixi/pull/3396)


#### Fixed

- Upgrade zip to 2.4.2 by @gzm55 in [#3389](https://github.com/prefix-dev/pixi/pull/3389)
- Improve shell execution by covering more edge-cases by @wolfv in [#3321](https://github.com/prefix-dev/pixi/pull/3321)
- Stop requiring `PATH` for `pixi global` activation by @Hofer-Julian in [#3403](https://github.com/prefix-dev/pixi/pull/3403)
- Improve error message for python integration tests by @Hofer-Julian in [#3408](https://github.com/prefix-dev/pixi/pull/3408)


#### New Contributors
* @trim21 made their first contribution in [#3386](https://github.com/prefix-dev/pixi/pull/3386)

### [0.43.0] - 2025-03-20
#### ✨ Highlights

- 🚀 Added support for [shortcuts](https://pixi.sh/v0.43.0/global_tools/introduction/#shortcuts) in `pixi global`.
- 🔄 Introduced [`pixi reinstall`](https://pixi.sh/v0.43.0/reference/cli/pixi/reinstall/).
- 🗃️ Enabled [sharded repodata](https://prefix.dev/blog/sharded_repodata) by default.
- 📚 Big documentation improvements
  - Restructuring of chapters
  - Generated [CLI docs](https://pixi.sh/v0.43.0/reference/cli/pixi/)
  - Renaming `[project]` to `[workspace]`

#### Added

- Support for shortcuts in `pixi global` by @Hofer-Julian in [#3226](https://github.com/prefix-dev/pixi/pull/3226)
- Add tutorial for pixi build rattler build by @nichmor in [#3330](https://github.com/prefix-dev/pixi/pull/3330)
- Add `pixi reinstall` by @Hofer-Julian in [#3344](https://github.com/prefix-dev/pixi/pull/3344)
- Add `pixi global shortcut` CLI by @ruben-arts in [#3341](https://github.com/prefix-dev/pixi/pull/3341)
- `pixi exec --list` by @lucascolley in [#3311](https://github.com/prefix-dev/pixi/pull/3311)
- Autogeneration of the CLI documentation. by @ruben-arts in [#3268](https://github.com/prefix-dev/pixi/pull/3268)


#### Changed

- Default to "workspace" with `pixi init` by @Hofer-Julian in [#3277](https://github.com/prefix-dev/pixi/pull/3277)
- Enable sharded repodata by default by @ruben-arts in [#3285](https://github.com/prefix-dev/pixi/pull/3285)


#### Documentation

- Set sso_region in s3 config example by @pavelzw in [#3289](https://github.com/prefix-dev/pixi/pull/3289)
- Refactor by @Hofer-Julian in [#3265](https://github.com/prefix-dev/pixi/pull/3265)
- Adjust keep in sync note by @pavelzw in [#3297](https://github.com/prefix-dev/pixi/pull/3297)
- Rename project to workspace by @Hofer-Julian in [#3300](https://github.com/prefix-dev/pixi/pull/3300)
- Use consistently "Pixi" instead of "pixi" by @Hofer-Julian in [#3302](https://github.com/prefix-dev/pixi/pull/3302)
- More Pixi capitalistation by @lucascolley in [#3304](https://github.com/prefix-dev/pixi/pull/3304)
- Update README.md to note PATH changes go in `~/.bashrc`, not `~/.bash_profile` by @petebachant in [#3305](https://github.com/prefix-dev/pixi/pull/3305)
- Clarify pixi spelling by @Hofer-Julian in [#3363](https://github.com/prefix-dev/pixi/pull/3363)
- Fix typo by @pavelzw in [#3385](https://github.com/prefix-dev/pixi/pull/3385)


#### Fixed

- Run test_build_git_source_deps_from_tag again for Windows by @Hofer-Julian in [#3273](https://github.com/prefix-dev/pixi/pull/3273)
- Print all tasks for the current platform in `pixi info` by @ruben-arts in [#3284](https://github.com/prefix-dev/pixi/pull/3284)
- Fix "extra-slow" snapshot by @Hofer-Julian in [#3298](https://github.com/prefix-dev/pixi/pull/3298)
- `pixi global install --force-reinstall` by @Hofer-Julian in [#3307](https://github.com/prefix-dev/pixi/pull/3307)
- Mark more tests as "slow" by @Hofer-Julian in [#3310](https://github.com/prefix-dev/pixi/pull/3310)
- Mark `test_pixi_auth` as extra slow by @Hofer-Julian in [#3309](https://github.com/prefix-dev/pixi/pull/3309)
- Use --locked to run the generation of CLI docs by @ruben-arts in [#3322](https://github.com/prefix-dev/pixi/pull/3322)
- Make --git work with --branch again by @ruben-arts in [#3323](https://github.com/prefix-dev/pixi/pull/3323)
- Don't prematurely shutdown runtime by @baszalmstra in [#3325](https://github.com/prefix-dev/pixi/pull/3325)
- Let amend_pypi_purls() always use reqwest client with the mirror middle ware by @gzm55 in [#3336](https://github.com/prefix-dev/pixi/pull/3336)
- Update build backend configuration by @nichmor in [#3352](https://github.com/prefix-dev/pixi/pull/3352)
- Use new windows runners with the dev drive feature by @ruben-arts in [#3361](https://github.com/prefix-dev/pixi/pull/3361)
- Fix winget releaser job by @ruben-arts in [#3380](https://github.com/prefix-dev/pixi/pull/3380)
- Use windows latest as arm build runner by @ruben-arts in [#3378](https://github.com/prefix-dev/pixi/pull/3378)
- Use compilers instead of custom set of dependencies by @ruben-arts in [#3337](https://github.com/prefix-dev/pixi/pull/3337)
- Some unexpected behavior of `pixi global update` by @Glatzel in [#3373](https://github.com/prefix-dev/pixi/pull/3373)


#### Refactor

- Replace ignore crate with pixi_glob by @prsabahrami in [#3333](https://github.com/prefix-dev/pixi/pull/3333)
- Pypi mapping into cached client by @baszalmstra in [#3318](https://github.com/prefix-dev/pixi/pull/3318)


#### Removed

- Remove conda-build protocol by @nichmor in [#3349](https://github.com/prefix-dev/pixi/pull/3349)


#### New Contributors
* @Glatzel made their first contribution in [#3373](https://github.com/prefix-dev/pixi/pull/3373)
* @prsabahrami made their first contribution in [#3333](https://github.com/prefix-dev/pixi/pull/3333)
* @gzm55 made their first contribution in [#3336](https://github.com/prefix-dev/pixi/pull/3336)
* @petebachant made their first contribution in [#3305](https://github.com/prefix-dev/pixi/pull/3305)

### [0.42.1] - 2025-03-05
#### Fixed

- Ignore `__archspec` in virtual package matching by @ruben-arts in [#3281](https://github.com/prefix-dev/pixi/pull/3281)

### [0.42.0] - 2025-03-04
#### ✨ Highlights
This release introduces an improved way of [dealing with Virtual Packages](https://github.com/prefix-dev/pixi/pull/2849).
For example, previously pixi would not allow this configuration on a non-CUDA machine:
```toml
[system-requirements]
cuda = "12"

[dependencies]
python = "*"
```
Now this setup also works on non-CUDA machines, because it only stops if the packages themselves actually depend on CUDA.
This is a first step to make the use of system-requirements/virtual-packages more flexible.

#### Changed

- Validate machine using lockfile by @ruben-arts in [#2849](https://github.com/prefix-dev/pixi/pull/2849)
- Upgrade `uv` crates to `0.6.1` by @nichmor in [#3216](https://github.com/prefix-dev/pixi/pull/3216)

#### Documentation

- Add s3 upload section by @pavelzw in [#3168](https://github.com/prefix-dev/pixi/pull/3168)
- Document cmake build backend `extra-args` by @wolfv in [#3167](https://github.com/prefix-dev/pixi/pull/3167)
- Sync pixi-pack docs with PR #104 (add --use-cache) by @awesomebytes in [#3189](https://github.com/prefix-dev/pixi/pull/3189)
- Mention add_pip_as_python_dependency in compatibility mode for pixi-pack by @pavelzw in [#3188](https://github.com/prefix-dev/pixi/pull/3188)
- Mention run-install in setup-pixi documentation by @pavelzw in [#3185](https://github.com/prefix-dev/pixi/pull/3185)
- Update python.md by @carschandler in [#3220](https://github.com/prefix-dev/pixi/pull/3220)
- Add section about using `pixi exec` in shebangs by @pavelzw in [#3201](https://github.com/prefix-dev/pixi/pull/3201)
- Fix snake_case config note by @salim-b in [#3232](https://github.com/prefix-dev/pixi/pull/3232)
- Fix note about config locations by @salim-b in [#3231](https://github.com/prefix-dev/pixi/pull/3231)
- Fix note formatting by @salim-b in [#3230](https://github.com/prefix-dev/pixi/pull/3230)
- Improve looks of a link by @ruben-arts in [#3229](https://github.com/prefix-dev/pixi/pull/3229)
- Add rattler-index s3 documentation by @pavelzw in [#3175](https://github.com/prefix-dev/pixi/pull/3175)
- Minor fix (three -> four) by @tylere in [#3240](https://github.com/prefix-dev/pixi/pull/3240)
- Add documentation about starship by @pavelzw in [#3242](https://github.com/prefix-dev/pixi/pull/3242)
- Add missing docs for `pixi install --all` by @Hofer-Julian in [#3256](https://github.com/prefix-dev/pixi/pull/3256)
- Typo in cli.md by @boisgera in [#3210](https://github.com/prefix-dev/pixi/pull/3210)

#### Fixed

- Improve UX on missing platforms by @ruben-arts in [#3169](https://github.com/prefix-dev/pixi/pull/3169)
- Allow usage of `--branch` `--tag` `--rev` for `--pypi` with `git` by @nichmor in [#3132](https://github.com/prefix-dev/pixi/pull/3132)
- Flush `shell` activation file after writing by @wolfv in [#3130](https://github.com/prefix-dev/pixi/pull/3130)
- Improve `pixi list` output for conda source packages by @ruben-arts in [#3170](https://github.com/prefix-dev/pixi/pull/3170)
- Pixi global revert error message by @Hofer-Julian in [#3207](https://github.com/prefix-dev/pixi/pull/3207)
- Return err when `pixi list` doesn't find pkg by @savente93 in [#3212](https://github.com/prefix-dev/pixi/pull/3212)
- Add caching of regular build by @tdejager in [#3238](https://github.com/prefix-dev/pixi/pull/3238)

#### Refactor

- Moving of types, renames and error updates by @tdejager in [#3228](https://github.com/prefix-dev/pixi/pull/3228)

#### New Contributors
* @salim-b made their first contribution in [#3230](https://github.com/prefix-dev/pixi/pull/3230)
* @savente93 made their first contribution in [#3212](https://github.com/prefix-dev/pixi/pull/3212)
* @boisgera made their first contribution in [#3210](https://github.com/prefix-dev/pixi/pull/3210)
* @xiaoxiangmoe made their first contribution in [#3191](https://github.com/prefix-dev/pixi/pull/3191)
* @awesomebytes made their first contribution in [#3189](https://github.com/prefix-dev/pixi/pull/3189)

### [0.41.4] - 2025-02-18
#### ✨ Highlights
This release add support for S3 backends.
You can configure a custom S3 backend in your `pixi.toml` file.
This allows you to use a custom S3 bucket as a channel for your project.

```toml
# pixi.toml
[project]
channels = ["s3://my-bucket/custom-channel"]

[project.s3-options.my-bucket]
endpoint-url = "https://my-s3-host"
region = "us-east-1"
force-path-style = false
```

#### Changed

- Implement `package.build.configuration` parsing by @wolfv in [#3115](https://github.com/prefix-dev/pixi/pull/3115)
- Add S3 backend support by @delsner in [#2825](https://github.com/prefix-dev/pixi/pull/2825)

#### Documentation

- Add S3 documentation by @pavelzw in [#2835](https://github.com/prefix-dev/pixi/pull/2835)
- Document git dependencies in pixi build documentation by @nichmor in [#3126](https://github.com/prefix-dev/pixi/pull/3126)

#### Fixed

- Manually exposed executables are removed after `pixi global update` by @Hofer-Julian in [#3109](https://github.com/prefix-dev/pixi/pull/3109)
- Changing cmake doesn't trigger rebuild by @Hofer-Julian in [#3102](https://github.com/prefix-dev/pixi/pull/3102)
- `BUILD_EDITABLE_PYTHON` env flag by @Hofer-Julian in [#3128](https://github.com/prefix-dev/pixi/pull/3128)
- Change the progress message during mapping of packages by @tdejager in [#3155](https://github.com/prefix-dev/pixi/pull/3155)
- Skip unneeded url parse and only add git+ when needed by @ruben-arts in [#3139](https://github.com/prefix-dev/pixi/pull/3139)
- Reinstall if required is source and installed is from registry by @ruben-arts in [#3131](https://github.com/prefix-dev/pixi/pull/3131)

### [0.41.3] - 2025-02-12
#### Changed
- Added `--dry-run` flag to pixi run by @noamgot in [#3107](https://github.com/prefix-dev/pixi/pull/3107)

#### Fixed
- Make prefix creation during solve thread-safe by @nichmor in [#3099](https://github.com/prefix-dev/pixi/pull/3099)
- Passing a file as `--manifest-path` by @tdejager in [#3111](https://github.com/prefix-dev/pixi/pull/3111)

#### New Contributors
* @noamgot made their first contribution in [#3107](https://github.com/prefix-dev/pixi/pull/3107)

### [0.41.2] - 2025-02-11
#### ✨ Highlights

This release introduces the ability to add environment variables to the `init --import` command.
We also upgraded the `uv` crate to `v0.5.29`.

#### Changed

- Add environment variables to `init --import` by @zklaus in [#3083](https://github.com/prefix-dev/pixi/pull/3083)
- Upgrade uv to `v0.5.29` by @tdejager in [#3075](https://github.com/prefix-dev/pixi/pull/3075)

#### Documentation

- Add Bodo to Community.md by @IsaacWarren in [#3103](https://github.com/prefix-dev/pixi/pull/3103)

#### Fixed

- Json Schema by @Hofer-Julian in [#3082](https://github.com/prefix-dev/pixi/pull/3082)
- Getting records for wrong platform by @tdejager in [#3084](https://github.com/prefix-dev/pixi/pull/3084)

#### Refactor

- Split workspace and package manifests by @baszalmstra in [#3043](https://github.com/prefix-dev/pixi/pull/3043)
- Env module by @tdejager in [#3074](https://github.com/prefix-dev/pixi/pull/3074)

#### New Contributors

* @IsaacWarren made their first contribution in [#3103](https://github.com/prefix-dev/pixi/pull/3103)
* @zklaus made their first contribution in [#3083](https://github.com/prefix-dev/pixi/pull/3083)

### [0.41.1] - 2025-02-07
#### Fixed
- Pixi authentication by @ruben-arts in [#3070](https://github.com/prefix-dev/pixi/pull/3070)

### [0.41.0] - 2025-02-05
#### ✨ Highlights

This release introduces lazily creating solve environments for the `pypi-dependencies` resulting in a significant speedup for environments that only depend on wheels.
If you want to force the use of wheels you can now also set `no-build` in the `pypi-options` table.
To test this you can now just use `pixi lock` to create a lockfile without installing an environment.

#### Added

- Add `pixi lock` by @Hofer-Julian in [#3061](https://github.com/prefix-dev/pixi/pull/3061) and [#3064](https://github.com/prefix-dev/pixi/pull/3064)
- Add `no-build` to `pypi-options` by @tdejager in [#2997](https://github.com/prefix-dev/pixi/pull/2997)

#### Changed

- Lazily initiate solve environments for `pypi-dependencies` by @nichmor and @tdejager in [#3029](https://github.com/prefix-dev/pixi/pull/3029)
- Use Github Releases `/latest` for `self-update` and prompt on downgrades by @jaimergp in [#2989](https://github.com/prefix-dev/pixi/pull/2989)
- Set PS1 in shell-hook too by @jaimergp in [#2991](https://github.com/prefix-dev/pixi/pull/2991)
- Prevent line-based 3-way merge on pixi.lock by @ChristianRothQC in [#3019](https://github.com/prefix-dev/pixi/pull/3019)
- Auto-prepend 'v' to version numbers during pixi installation by @m-naumann in [#3000](https://github.com/prefix-dev/pixi/pull/3000)
- Parse labels and related errors from build backend by @baszalmstra in [#2952](https://github.com/prefix-dev/pixi/pull/2952)

#### Documentation

- Enable autocomplete on Zsh by @ywilke in [#3009](https://github.com/prefix-dev/pixi/pull/3009)
- Add section on aws codeartifact by @rayduck in [#3020](https://github.com/prefix-dev/pixi/pull/3020)
- Add `pixi-diff` documentation by @pavelzw in [#3025](https://github.com/prefix-dev/pixi/pull/3025)
- Fix python tutorial by @ruben-arts in [#3035](https://github.com/prefix-dev/pixi/pull/3035)

#### Fixed

- Always show cursor after control+c by @ruben-arts in [#2635](https://github.com/prefix-dev/pixi/pull/2635)
- `mirrors` configuration follows correct priority by @ruben-arts in [#3002](https://github.com/prefix-dev/pixi/pull/3002)
- Don't check requires python in satisfiability by @ruben-arts in [#2968](https://github.com/prefix-dev/pixi/pull/2968)
- Enforce recompile trampoline by @Hofer-Julian in [#3013](https://github.com/prefix-dev/pixi/pull/3013)
- `project platform remove` by @Hofer-Julian in [#3017](https://github.com/prefix-dev/pixi/pull/3017)
- Lockfile not invalidated when we remove platforms by @Hofer-Julian in [#3027](https://github.com/prefix-dev/pixi/pull/3027)
- Will also update prefix if there are pypi path dependencies by @tdejager in [#3034](https://github.com/prefix-dev/pixi/pull/3034)
- Initialize rayon late and use uv from tag by @baszalmstra in [#2957](https://github.com/prefix-dev/pixi/pull/2957)
- Change back to multithreaded runtime by @tdejager in [#3041](https://github.com/prefix-dev/pixi/pull/3041)
- Logic was backward for creating missing .bashrc by @hjmjohnson in [#3051](https://github.com/prefix-dev/pixi/pull/3051)
- Do proper sanity check on add and run by @ruben-arts in [#3054](https://github.com/prefix-dev/pixi/pull/3054)
- Add check error message to remove uv dependencies by @Dozie2001 in [#3052](https://github.com/prefix-dev/pixi/pull/3052)

#### Refactor

- Make diff module public by @pavelzw in [#3010](https://github.com/prefix-dev/pixi/pull/3010)
- Enforce no `unwrap` via clippy by @Hofer-Julian in [#3006](https://github.com/prefix-dev/pixi/pull/3006)
- Improve AuthenticationStorage, update rattler by @pavelzw in [#2909](https://github.com/prefix-dev/pixi/pull/2909)

#### Removed

- Remove `description` from `pixi init` template by @Hofer-Julian in [#3012](https://github.com/prefix-dev/pixi/pull/3012)

#### New Contributors
* @Dozie2001 made their first contribution in [#3052](https://github.com/prefix-dev/pixi/pull/3052)
* @hjmjohnson made their first contribution in [#3051](https://github.com/prefix-dev/pixi/pull/3051)
* @m-naumann made their first contribution in [#3000](https://github.com/prefix-dev/pixi/pull/3000)
* @ChristianRothQC made their first contribution in [#3019](https://github.com/prefix-dev/pixi/pull/3019)
* @rayduck made their first contribution in [#3020](https://github.com/prefix-dev/pixi/pull/3020)
* @ywilke made their first contribution in [#3009](https://github.com/prefix-dev/pixi/pull/3009)

### [0.40.3] - 2025-01-22
#### ✨ Highlights
This release will greatly improve the `git` dependency experience for PyPI packages.

#### Added
- Add nushell autocompletion for pixi r by @dennis-wey in [#2935](https://github.com/prefix-dev/pixi/pull/2935)

#### Changed
- Pin backend versions by @tdejager in [#2963](https://github.com/prefix-dev/pixi/pull/2963)

#### Documentation
- Add `quantity-array` to community by @lucascolley in [#2955](https://github.com/prefix-dev/pixi/pull/2955)
- Add multiple environment tutorial by @ruben-arts in [#2949](https://github.com/prefix-dev/pixi/pull/2949)
- Use workspace channels for build tutorials by @Hofer-Julian in [#2940](https://github.com/prefix-dev/pixi/pull/2940)
- Fix ambiguous version specifiers by @Hofer-Julian in [#2967](https://github.com/prefix-dev/pixi/pull/2967)
- Fix broken links to anchors by @Hofer-Julian in [#2941](https://github.com/prefix-dev/pixi/pull/2941)

#### Fixed
- Fix `branch`, `tag` and `rev` for `pypi-dependencies` by @nichmor in [#2960](https://github.com/prefix-dev/pixi/pull/2960)
- `pixi list` should print the git location instead of the wheel by @ruben-arts in [#2962](https://github.com/prefix-dev/pixi/pull/2962)
- Improve debuggability of the list output by @ruben-arts in [#2975](https://github.com/prefix-dev/pixi/pull/2975)
- Also warn about detached environments on Windows by @Hofer-Julian in [#2985](https://github.com/prefix-dev/pixi/pull/2985)
- Fix binaries for linux-aarch64 by @ruben-arts in [#2937](https://github.com/prefix-dev/pixi/pull/2937)

#### Refactor
- Use destructuring to remove clones in conversion by @KGrewal1 in [#2969](https://github.com/prefix-dev/pixi/pull/2969)

### [0.40.2] - 2025-01-17
#### Added
- Add a progress bar for source ( git ) dependencies by @nichmor in [#2898](https://github.com/prefix-dev/pixi/pull/2898)

#### Changed
- Split into 'source' and 'binary' build types by @tdejager in [#2903](https://github.com/prefix-dev/pixi/pull/2903)

#### Documentation
- Update index.md - windows install command by @raybellwaves in [#2871](https://github.com/prefix-dev/pixi/pull/2871)
- Fix `project_model` module docs by @Hofer-Julian in [#2928](https://github.com/prefix-dev/pixi/pull/2928)
- Pixi build variants by @baszalmstra in [#2901](https://github.com/prefix-dev/pixi/pull/2901)

#### Fixed
- CamelCase project protocol types by @baszalmstra in [#2907](https://github.com/prefix-dev/pixi/pull/2907)
- Rewrite prefix guard into async by @nichmor in [#2908](https://github.com/prefix-dev/pixi/pull/2908)
- Double_lines in copy of docs by @ruben-arts in [#2913](https://github.com/prefix-dev/pixi/pull/2913)
- Stackoverflow when running pixi in debug mode on windows by @baszalmstra in [#2922](https://github.com/prefix-dev/pixi/pull/2922)
- `pixi run --help` by @Hofer-Julian in [#2918](https://github.com/prefix-dev/pixi/pull/2918)
- Shell hang on progress bar by @baszalmstra in [#2929](https://github.com/prefix-dev/pixi/pull/2929)
- Take into account the variants for the source cache by @baszalmstra in [#2877](https://github.com/prefix-dev/pixi/pull/2877)
- Pixi init by @Hofer-Julian in [#2930](https://github.com/prefix-dev/pixi/pull/2930)

#### New Contributors
* @raybellwaves made their first contribution in [#2871](https://github.com/prefix-dev/pixi/pull/2871)

### [0.40.1] - 2025-01-14
#### ✨ Highlights
We've **reverted** the breaking change of the `depends_on` field from `0.40.0`, replacing it with a [warning](https://github.com/prefix-dev/pixi/pull/2891).

This release also brings a [performance boost](https://github.com/prefix-dev/pixi/pull/2874) to our Windows and Linux-musl builds by using faster allocators.
On the ([holoviews](https://github.com/holoviz/holoviews)) project, we measured a significant speedup:
```shell
# Linux musl
Summary
  pixi-0.40.1 list --no-install ran
   12.65 ± 0.46 times faster than pixi-0.40.0 list --no-install

# Windows
  pixi-0.40.1 list --no-install ran
    1.66 ± 0.07 times faster than pixi-0.40.0 list --no-install
    1.67 ± 0.09 times faster than pixi-0.39.5 list --no-install
    2.10 ± 0.09 times faster than pixi-0.39.4 list --no-install
```

#### Fixed
- Pyproject `entry-points` by @atmorling in [#2886](https://github.com/prefix-dev/pixi/pull/2886)
- Print warning when pixi manifest is not parsed in pixi search by @pavelzw in [#2889](https://github.com/prefix-dev/pixi/pull/2889)
- Add deprecation notice for `depends_on` by @baszalmstra in [#2891](https://github.com/prefix-dev/pixi/pull/2891)

#### Performance
- Use faster allocators by @baszalmstra in [#2874](https://github.com/prefix-dev/pixi/pull/2874)

#### Refactor
- Add `online_tests` feature to control Internet use by @mgorny in [#2881](https://github.com/prefix-dev/pixi/pull/2881)
- Simplify repodata_gateway function by @olivier-lacroix in [#1793](https://github.com/prefix-dev/pixi/pull/1793)
- Spawn main entrypoint in box by @baszalmstra in [#2892](https://github.com/prefix-dev/pixi/pull/2892)

#### New Contributors
* @atmorling made their first contribution in [#2886](https://github.com/prefix-dev/pixi/pull/2886)
* @mgorny made their first contribution in [#2881](https://github.com/prefix-dev/pixi/pull/2881)

### [0.40.0] - 2025-01-10
#### ✨ Highlights
Manifest file parsing has been significantly improved.
Errors will now be clearer and more helpful, for example:
```shell
         × Expected one of 'first-index', 'unsafe-first-match', 'unsafe-best-match'
          ╭─[pixi.toml:2:27]
        1 │
        2 │         index-strategy = "UnsafeFirstMatch"
          ·                           ────────────────
        3 │
          ╰────
         help: Did you mean 'unsafe-first-match'?
```

#### Breaking Change Alert:
The `depends_on` field has been renamed to `depends-on` for better consistency.
Using the old format without a dash (depends_on) will now result in an error.
The new errors should help you find the location:
```shell
Error:
  × field 'depends_on' is deprecated, 'depends-on' has replaced it
    ╭─[pixi.toml:22:51]
 21 │ install = "cargo install --path . --locked"
 22 │ install-as = { cmd = "python scripts/install.py", depends_on = [
    ·                                                   ─────┬────
    ·                                                        ╰── replace this with 'depends-on'
 23 │   "build-release",
    ╰────
```

#### Added
- Pixi add git source dependency by @nichmor in [#2858](https://github.com/prefix-dev/pixi/pull/2858)

#### Documentation
- Fix installation docs mistake in index.md by @PanTheDev in [#2869](https://github.com/prefix-dev/pixi/pull/2869)

#### Fixed
- Create missing global manifest folder with pixi global edit by @zbowling in [#2847](https://github.com/prefix-dev/pixi/pull/2847)
- Pixi add creates a project by @nichmor in [#2861](https://github.com/prefix-dev/pixi/pull/2861)
- Initialized detached envs with None by @ruben-arts in [#2841](https://github.com/prefix-dev/pixi/pull/2841)

#### `pixi build` Preview work
- Build backend docs by @tdejager in [#2844](https://github.com/prefix-dev/pixi/pull/2844)
- Move pixi build type conversions into its own crate by @tdejager in [#2866](https://github.com/prefix-dev/pixi/pull/2866)
- Expose build type v1 function by @tdejager in [#2875](https://github.com/prefix-dev/pixi/pull/2875)
- Use toml-span for deserialization by @baszalmstra in [#2718](https://github.com/prefix-dev/pixi/pull/2718)
- Expands options for setting pixi-build override options by @tdejager in [#2843](https://github.com/prefix-dev/pixi/pull/2843)
- Split capability retrieval from initialize by @tdejager in [#2831](https://github.com/prefix-dev/pixi/pull/2831)
- Move package fields under `[package]`. by @baszalmstra in [#2731](https://github.com/prefix-dev/pixi/pull/2731)
- Extract pixi manifest info into protocol by @tdejager in [#2850](https://github.com/prefix-dev/pixi/pull/2850)

#### New Contributors
* @PanTheDev made their first contribution in [#2869](https://github.com/prefix-dev/pixi/pull/2869)

### [0.39.5] - 2025-01-06
#### ✨ Highlights
By updating [`resolvo`](https://github.com/mamba-org/resolvo/pull/91) to the latest version we now significantly lower the RAM usage during the solve process. 🚀
As this improvement removes a huge set of data from the solve step it also speeds it up even more, especially for hard to solve environments.

Some numbers from the `resolvo` PR, based on the resolve test dataset:
```
- Average Solve Time: 'pixi v0.39.5' was 1.68 times faster than 'pixi v0.39.4'
- Median Solve Time: 'pixi v0.39.5' was 1.33 times faster than 'pixi v0.39.4'
- 25th Percentile: 'pixi v0.39.5' was 1.22 times faster than 'pixi v0.39.4'
- 75th Percentile: 'pixi v0.39.5' was 2.28 times faster than 'pixi v0.39.4'
```

#### Added
- Add cli modifications of the system requirements by @ruben-arts in [#2765](https://github.com/prefix-dev/pixi/pull/2765)
- Support `--manifest-path` to project directory by @blmaier in [#2716](https://github.com/prefix-dev/pixi/pull/2716)

#### Changed
- Make binary, config folder, and lock file names dynamic by @zbowling in [#2775](https://github.com/prefix-dev/pixi/pull/2775)

#### Documentation
- Add `marray` to community by @lucascolley in [#2774](https://github.com/prefix-dev/pixi/pull/2774)
- Simplify nushell completion script by @Hofer-Julian in [#2782](https://github.com/prefix-dev/pixi/pull/2782)
- Fix typo in PyCharm integration doc by @stevenschaerer in [#2766](https://github.com/prefix-dev/pixi/pull/2766)
- Do not depend on gxx in pixi build docs by @traversaro in [#2815](https://github.com/prefix-dev/pixi/pull/2815)
- Fix typo by @pavelzw in [#2833](https://github.com/prefix-dev/pixi/pull/2833)

#### Fixed
- Move away from lazy_static by @Hofer-Julian in [#2781](https://github.com/prefix-dev/pixi/pull/2781)
- Don't modify manifest on failing `pixi add/upgrade` by @ruben-arts in [#2756](https://github.com/prefix-dev/pixi/pull/2756)
- Ignore .pixi folder for build by @baszalmstra in [#2801](https://github.com/prefix-dev/pixi/pull/2801)
- Use correct directory for build artifact cache by @baszalmstra in [#2830](https://github.com/prefix-dev/pixi/pull/2830)
- Detect Freethreading Python by @nichmor in [#2762](https://github.com/prefix-dev/pixi/pull/2762)


#### New Contributors
* @stevenschaerer made their first contribution in [#2766](https://github.com/prefix-dev/pixi/pull/2766)
* @zbowling made their first contribution in [#2775](https://github.com/prefix-dev/pixi/pull/2775)

### [0.39.4] - 2024-12-23
#### ✨ Highlights
Last release got an additional speedup for macOS specifically! 🚀

#### Performance
- Speedup environment installation on macOS by @wolfv in [#2754](https://github.com/prefix-dev/pixi/pull/2701)

#### Added
- Add script to build trampolines by @Hofer-Julian in [#2752](https://github.com/prefix-dev/pixi/pull/2752)

#### Changed
- Serialize system requirements by @ruben-arts in [#2753](https://github.com/prefix-dev/pixi/pull/2753)

#### Documentation
- Add pytorch integration guide. by @ruben-arts in [#2711](https://github.com/prefix-dev/pixi/pull/2711)

#### Fixed
- Rename the ppc binary archive by @ruben-arts in [#2750](https://github.com/prefix-dev/pixi/pull/2750)
- Retry on http failures by @Hofer-Julian in [#2755](https://github.com/prefix-dev/pixi/pull/2755)

#### `pixi build` Preview work
- Update reference for pixi build by @Hofer-Julian in [#2735](https://github.com/prefix-dev/pixi/pull/2735)
- Add tutorial for pixi build workspace by @Hofer-Julian in [#2727](https://github.com/prefix-dev/pixi/pull/2727)
- Add support for git source dependencies in pixi build by @nichmor in [#2680](https://github.com/prefix-dev/pixi/pull/2680)

### [0.39.3] - 2024-12-18
#### ✨ Highlights
This release includes a little Christmas present, the environment installation got a huge speedup! 🚀

#### Performance
- Speedup environment installation by @baszalmstra and @wolfv in [#2701](https://github.com/prefix-dev/pixi/pull/2701)

#### Fixed
- `pixi global sync` reports after each handled environment by @Hofer-Julian in [#2698](https://github.com/prefix-dev/pixi/pull/2698)
- Config search order by @Hofer-Julian in [#2702](https://github.com/prefix-dev/pixi/pull/2702)
- Enforce rust-tls for reqwest by @Hofer-Julian in [#2719](https://github.com/prefix-dev/pixi/pull/2719)
- Help user with lockfile update error by @ruben-arts in [#2684](https://github.com/prefix-dev/pixi/pull/2684)
- Add broken curl version check in install.sh by @thewtex in [#2686](https://github.com/prefix-dev/pixi/pull/2686)
- Avoid race condition on bariercell when future is instant by @baszalmstra in [#2736](https://github.com/prefix-dev/pixi/pull/2736)
- Log config parsing errors as errors by @Hofer-Julian in [#2739](https://github.com/prefix-dev/pixi/pull/2739)

#### `pixi build` Preview work
- Introduction to pixi build by @tdejager in [#2685](https://github.com/prefix-dev/pixi/pull/2685)
- Add community example to ROS2 tutorial by @Daviesss in [#2713](https://github.com/prefix-dev/pixi/pull/2713)
- Add tutorial for Python and pixi-build by @Hofer-Julian in [#2715](https://github.com/prefix-dev/pixi/pull/2715)
- C++ package pixi build example by @tdejager in [#2717](https://github.com/prefix-dev/pixi/pull/2717)
- Add target to workspace by @wolfv in [#2655](https://github.com/prefix-dev/pixi/pull/2655)
- Support editable install for `pixi build` by @Hofer-Julian in [#2661](https://github.com/prefix-dev/pixi/pull/2661)

#### New Contributors
* @Daviesss made their first contribution in [#2713](https://github.com/prefix-dev/pixi/pull/2713)
* @thewtex made their first contribution in [#2686](https://github.com/prefix-dev/pixi/pull/2686)

### [0.39.2] - 2024-12-11
Patch release to fix the binary generation in CI.

### [0.39.1] - 2024-12-09
#### Added

- Add proper unit testing for PyPI installation and fix re-installation issues by @tdejager in [#2617](https://github.com/prefix-dev/pixi/pull/2617)
- Add detailed json output for task list by @jjjermiah in [#2608](https://github.com/prefix-dev/pixi/pull/2608)
- Add `pixi project name` CLI by @LiamConnors in [#2649](https://github.com/prefix-dev/pixi/pull/2649)

#### Changed

- Use `fs-err` in more places by @Hofer-Julian in [#2636](https://github.com/prefix-dev/pixi/pull/2636)

#### Documentation

- Remove `tclf` from community.md📑 by @KarelZe in [#2619](https://github.com/prefix-dev/pixi/pull/2619)
- Update contributing guide by @LiamConnors in [#2650](https://github.com/prefix-dev/pixi/pull/2650)
- Update clean cache CLI doc by @LiamConnors in [#2657](https://github.com/prefix-dev/pixi/pull/2657)

#### Fixed

- Color formatting detection on stdout by @blmaier in [#2613](https://github.com/prefix-dev/pixi/pull/2613)
- Use correct dependency location for `pixi upgrade` by @Hofer-Julian in [#2472](https://github.com/prefix-dev/pixi/pull/2472)
- Regression `detached-environments` not used by @ruben-arts in [#2627](https://github.com/prefix-dev/pixi/pull/2627)
- Allow configuring pypi insecure host by @zen-xu in [#2521](https://github.com/prefix-dev/pixi/pull/2521)[#2622](https://github.com/prefix-dev/pixi/pull/2622)

#### Refactor

- Rework CI and use `cargo-dist` for releases by @baszalmstra in [#2566](https://github.com/prefix-dev/pixi/pull/2566)

#### `pixi build` Preview work
- Refactor to `[build-system.build-backend]` by @baszalmstra in [#2601](https://github.com/prefix-dev/pixi/pull/2601)
- Remove ipc override from options and give it manually to test by @wolfv in [#2629](https://github.com/prefix-dev/pixi/pull/2629)
- Pixi build trigger rebuild by @Hofer-Julian in [#2641](https://github.com/prefix-dev/pixi/pull/2641)
- Add variant config to `[workspace.build-variants]` by @wolfv in [#2634](https://github.com/prefix-dev/pixi/pull/2634)
- Add request coalescing for isolated tools by @nichmor in [#2589](https://github.com/prefix-dev/pixi/pull/2589)
- Add example using `rich` and `pixi-build-python` and remove flask by @Hofer-Julian in [#2638](https://github.com/prefix-dev/pixi/pull/2638)
- (simple) build tool override by @wolfv in [#2620](https://github.com/prefix-dev/pixi/pull/2620)
- Add caching of build tool installation by @nichmor in [#2637](https://github.com/prefix-dev/pixi/pull/2637)
#### New Contributors
* @blmaier made their first contribution in [#2613](https://github.com/prefix-dev/pixi/pull/2613)

### [0.39.0] - 2024-12-02
#### ✨ Highlights

- We now have a new `concurrency` configuration in the `pixi.toml` file.
This allows you to set the number of concurrent solves or downloads that can be run at the same time.
- We changed the way pixi searches for a pixi manifest. Where it was previously first considering the activated `pixi shell`, it will now search first in the current directory and its parent directories. [more info](https://github.com/prefix-dev/pixi/pull/2564)
- The lockfile format is changed to make it slightly smaller and support source dependencies.

#### Added

- Add `concurrency` configuration by @ruben-arts in [#2569](https://github.com/prefix-dev/pixi/pull/2569)

#### Changed

- Add `XDG_CONFIG_HOME`/`.config` to search of pixi global manifest path by @hoxbro in [#2547](https://github.com/prefix-dev/pixi/pull/2547)
- Let `pixi global sync` collect errors rather than returning early by @Hofer-Julian in [#2586](https://github.com/prefix-dev/pixi/pull/2586)
- Allow configuring pypi insecure host by @zen-xu in [#2521](https://github.com/prefix-dev/pixi/pull/2521)
- Reorder manifest discovery logic by @Hofer-Julian in [#2564](https://github.com/prefix-dev/pixi/pull/2564)

#### Documentation

- Improve pixi manifest by @Hofer-Julian in [#2596](https://github.com/prefix-dev/pixi/pull/2596)

#### Fixed

- `pixi global list` failing for empty environments by @Hofer-Julian in [#2571](https://github.com/prefix-dev/pixi/pull/2571)
- Macos activation cargo vars by @ruben-arts in [#2578](https://github.com/prefix-dev/pixi/pull/2578)
- Trampoline without corresponding json breaking by @Hofer-Julian in [#2576](https://github.com/prefix-dev/pixi/pull/2576)
- Ensure pinning strategy is not affected by non-semver packages by @seowalex in [#2580](https://github.com/prefix-dev/pixi/pull/2580)
- Pypi installs happening every time by @tdejager in [#2587](https://github.com/prefix-dev/pixi/pull/2587)
- `pixi global` report formatting by @Hofer-Julian in [#2595](https://github.com/prefix-dev/pixi/pull/2595)
- Improve test speed and support win-arm64 by @baszalmstra in [#2597](https://github.com/prefix-dev/pixi/pull/2597)
- Update Task::Alias to return command description by @jjjermiah in [#2607](https://github.com/prefix-dev/pixi/pull/2607)

#### Refactor

- Split install pypi into module and files by @tdejager in [#2590](https://github.com/prefix-dev/pixi/pull/2590)
- PyPI installation traits + deduplication by @tdejager in [#2599](https://github.com/prefix-dev/pixi/pull/2599)

#### Pixi build
We've merged in the main `pixi build` feature branch. This is a big change but shouldn't have affected any of the current functionality.
If you notice any issues, please let us know.

It can be turned on by `preview = "pixi-build"` in your `pixi.toml` file. It's under heavy development so expect breaking changes in that feature for now.

- Preview of `pixi build` and workspaces by @tdejager in [#2250](https://github.com/prefix-dev/pixi/pull/2250)
- Build recipe yaml directly by @wolfv in [#2568](https://github.com/prefix-dev/pixi/pull/2568)

#### New Contributors
* @seowalex made their first contribution in [#2580](https://github.com/prefix-dev/pixi/pull/2580)

### [0.38.0] - 2024-11-26
#### ✨ Highlights

- Specify `pypi-index` per pypi-dependency
```toml
[pypi-dependencies]
pytorch ={ version = "*", index = "https://download.pytorch.org/whl/cu118" }
```
- `[dependency-groups]` (PEP735) support in `pyproject.toml`
```toml
[dependency-groups]
test = ["pytest"]
docs = ["sphinx"]
dev = [{include-group = "test"}, {include-group = "docs"}]

[tool.pixi.environments]
dev = ["dev"]
```
- Much improved `pixi search` output!

#### Added
- Add pypi index by @nichmor in [#2416](https://github.com/prefix-dev/pixi/pull/2416)
- Implement PEP735 support by @olivier-lacroix in [#2448](https://github.com/prefix-dev/pixi/pull/2448)
- Extends manifest to allow for `preview` features by @tdejager in [#2489](https://github.com/prefix-dev/pixi/pull/2489)
- Add versions/build list to `pixi search` output by @delsner in [#2440](https://github.com/prefix-dev/pixi/pull/2440)
- Expose nested executables in `pixi global` by @bahugo in [#2362](https://github.com/prefix-dev/pixi/pull/2362)

#### Fixed

- Always print a warning when config is invalid by @Hofer-Julian in [#2508](https://github.com/prefix-dev/pixi/pull/2508)
- Incorrectly saving absolute base as path component by @tdejager in [#2501](https://github.com/prefix-dev/pixi/pull/2501)
- Keep the case when getting the executable in `pixi global` by @wolfv in [#2528](https://github.com/prefix-dev/pixi/pull/2528)
- Install script on `win-arm64` by @baszalmstra in [#2538](https://github.com/prefix-dev/pixi/pull/2538)
- Trampoline installation on `pixi global update` by @nichmor in [#2530](https://github.com/prefix-dev/pixi/pull/2530)
- Update the `PATH` env var with dynamic elements on `pixi global` by @wolfv in [#2541](https://github.com/prefix-dev/pixi/pull/2541)
- Correct `ppc64le` arch by @wolfv in [#2540](https://github.com/prefix-dev/pixi/pull/2540)

#### Performance

- Experimental environment activation cache by @ruben-arts in [#2367](https://github.com/prefix-dev/pixi/pull/2367)

#### Documentation
- Update project structure in Python tutorial by @LiamConnors in [#2506](https://github.com/prefix-dev/pixi/pull/2506)
- Fix typo in `pixi project export conda-environment` by @nmarticorena in [#2533](https://github.com/prefix-dev/pixi/pull/2533)
- Fix wrong use of underscores in `pixi project export` by @traversaro in [#2539](https://github.com/prefix-dev/pixi/pull/2539)
- Adapt completion instructions by @Hofer-Julian in [#2561](https://github.com/prefix-dev/pixi/pull/2561)

#### New Contributors
* @nmarticorena made their first contribution in [#2533](https://github.com/prefix-dev/pixi/pull/2533)
* @delsner made their first contribution in [#2440](https://github.com/prefix-dev/pixi/pull/2440)

### [0.37.0] - 2024-11-18
#### ✨ Highlights

We now allow the use of `prefix.dev` channels with sharded repodata:

Running `pixi search rubin-env` using `hyperfine` on the default versus our channels gives these results:

| Cache Status | Channel                                  | Mean [ms] | Relative |
|:-------------|------------------------------------------|----------:|---------:|
| With cache   | `https://prefix.dev/conda-forge`         |      69.3 |     1.00 |
| Without      | `https://prefix.dev/conda-forge`         |     389.5 |     5.62 |
| With cache   | `https://conda.anaconda.org/conda-forge` |    1043.3 |    15.06 |
| Without      | `https://conda.anaconda.org/conda-forge` |    2420.3 |    34.94 |

#### Breaking

- Make sure that `[activation.env]` are not completely overridden by `[target.` tables, by @hameerabbasi in [#2396](https://github.com/prefix-dev/pixi/pull/2396)

#### Changed

- Allow using sharded repodata by @baszalmstra in [#2467](https://github.com/prefix-dev/pixi/pull/2467)

#### Documentation

- Update ros2.md turtlesim section by @nbbrooks in [#2442](https://github.com/prefix-dev/pixi/pull/2442)
- Update pycharm.md to show optional installation by @plainerman in [#2487](https://github.com/prefix-dev/pixi/pull/2487)
- Fix typo in documentation by @saraedum in [#2496](https://github.com/prefix-dev/pixi/pull/2496)
- Update pixi install output by @LiamConnors in [#2495](https://github.com/prefix-dev/pixi/pull/2495)

#### Fixed

- Incorrect python version was used in some parts of the solve by @tdejager in [#2481](https://github.com/prefix-dev/pixi/pull/2481)
- Wrong description on pixi upgrade by @notPlancha in [#2483](https://github.com/prefix-dev/pixi/pull/2483)
- Extra test for mismatch in python versions by @tdejager in [#2485](https://github.com/prefix-dev/pixi/pull/2485)
- Keep `build` in `pixi upgrade` by @ruben-arts in [#2476](https://github.com/prefix-dev/pixi/pull/2476)

#### New Contributors
* @saraedum made their first contribution in [#2496](https://github.com/prefix-dev/pixi/pull/2496)
* @plainerman made their first contribution in [#2487](https://github.com/prefix-dev/pixi/pull/2487)
* @hameerabbasi made their first contribution in [#2396](https://github.com/prefix-dev/pixi/pull/2396)
* @nbbrooks made their first contribution in [#2442](https://github.com/prefix-dev/pixi/pull/2442)

### [0.36.0] - 2024-11-07
#### ✨ Highlights

- You can now `pixi upgrade` your project dependencies.
- We've done a performance improvement on the prefix validation check, thus faster `pixi run` startup times.

#### Added

- Add powerpc64le target to trampoline by @ruben-arts in [#2419](https://github.com/prefix-dev/pixi/pull/2419)
- Add trampoline tests again by @Hofer-Julian in [#2420](https://github.com/prefix-dev/pixi/pull/2420)
- Add `pixi upgrade` by @Hofer-Julian in [#2368](https://github.com/prefix-dev/pixi/pull/2368)
- Add platform fallback win-64 for win-arm64 by @chawyehsu in [#2427](https://github.com/prefix-dev/pixi/pull/2427)
- Add `--prepend` option for `pixi project channel add` by @mrswastik-robot in [#2447](https://github.com/prefix-dev/pixi/pull/2447)

#### Documentation

- Fix cli basic usage example by @lucascolley in [#2432](https://github.com/prefix-dev/pixi/pull/2432)
- Update python tutorial by @LiamConnors in [#2452](https://github.com/prefix-dev/pixi/pull/2452)
- Improve `pixi global` docs by @Hofer-Julian in [#2437](https://github.com/prefix-dev/pixi/pull/2437)

#### Fixed

- Use `--silent` instead of `--no-progress-meter` for old `curl` by @jaimergp in [#2428](https://github.com/prefix-dev/pixi/pull/2428)
- Search should return latest package across all platforms by @nichmor in [#2424](https://github.com/prefix-dev/pixi/pull/2424)
- Trampoline unwraps by @ruben-arts in [#2422](https://github.com/prefix-dev/pixi/pull/2422)
- PyPI Index usage (regression in v0.35.0) by @tdejager in [#2465](https://github.com/prefix-dev/pixi/pull/2465)
- PyPI git dependencies (regression in v0.35.0) by @wolfv in [#2438](https://github.com/prefix-dev/pixi/pull/2438)
- Tolerate pixi file errors (regression in v0.35.0) by @jvenant in [#2457](https://github.com/prefix-dev/pixi/pull/2457)
- Make sure tasks are fetched for best platform by @jjjermiah in [#2446](https://github.com/prefix-dev/pixi/pull/2446)

#### Performance

- Quick prefix validation check by @ruben-arts in [#2400](https://github.com/prefix-dev/pixi/pull/2400)

#### New Contributors
* @jvenant made their first contribution in [#2457](https://github.com/prefix-dev/pixi/pull/2457)
* @mrswastik-robot made their first contribution in [#2447](https://github.com/prefix-dev/pixi/pull/2447)
* @LiamConnors made their first contribution in [#2452](https://github.com/prefix-dev/pixi/pull/2452)


### [0.35.0] - 2024-11-05
#### ✨ Highlights

`pixi global` now exposed binaries are not scripts anymore but actual executables.
Resulting in significant speedup and better compatibility with other tools.

#### Added

- Add language packages with minor pinning by default by @ruben-arts in [#2310](https://github.com/prefix-dev/pixi/pull/2310)
- Add grouping for exposing and removing by @nichmor in [#2387](https://github.com/prefix-dev/pixi/pull/2387)
- Add trampoline for pixi global by @Hofer-Julian and @nichmor in [#2381](https://github.com/prefix-dev/pixi/pull/2381)
- Adding SCM option for init command by @alvgaona in [#2342](https://github.com/prefix-dev/pixi/pull/2342)
- Create `.pixi/.gitignore` containing `*` by @maresb in [#2361](https://github.com/prefix-dev/pixi/pull/2361)

#### Changed

- Use the same package cache folder by @nichmor in [#2335](https://github.com/prefix-dev/pixi/pull/2335)zx
- Disable progress in non tty by @ruben-arts in [#2308](https://github.com/prefix-dev/pixi/pull/2308)
- Improve global install reporting by @Hofer-Julian in [#2395](https://github.com/prefix-dev/pixi/pull/2395)
- Suggest fix in platform error message by @maurosilber in [#2404](https://github.com/prefix-dev/pixi/pull/2404)
- Upgrading uv to `0.4.30` by @tdejager in [#2372](https://github.com/prefix-dev/pixi/pull/2372)

#### Documentation

- Add pybind11 example by @alvgaona in [#2324](https://github.com/prefix-dev/pixi/pull/2324)
- Replace build with uv in pybind11 example by @alvgaona in [#2341](https://github.com/prefix-dev/pixi/pull/2341)
- Fix incorrect statement about env location by @opcode81 in [#2370](https://github.com/prefix-dev/pixi/pull/2370)

#### Fixed

- Global update reporting by @Hofer-Julian in [#2352](https://github.com/prefix-dev/pixi/pull/2352)
- Correctly display unrequested environments on `task list` by @jjjermiah in [#2402](https://github.com/prefix-dev/pixi/pull/2402)

#### Refactor

- Use built in string methods by @KGrewal1 in [#2348](https://github.com/prefix-dev/pixi/pull/2348)
- Reorganize integration tests by @Hofer-Julian in [#2408](https://github.com/prefix-dev/pixi/pull/2408)
- Reimplement barrier cell on OnceLock by @KGrewal1 in [#2347](https://github.com/prefix-dev/pixi/pull/2347)

#### New Contributors
* @maurosilber made their first contribution in [#2404](https://github.com/prefix-dev/pixi/pull/2404)
* @opcode81 made their first contribution in [#2370](https://github.com/prefix-dev/pixi/pull/2370)
* @alvgaona made their first contribution in [#2342](https://github.com/prefix-dev/pixi/pull/2342)

### [0.34.0] - 2024-10-21
#### ✨ Highlights

- `pixi global install` now takes a flag `--with`, inspired by `uv tool install`. If you only want to add dependencies without exposing them, you can now run `pixi global install ipython --with numpy --with matplotlib`
- Improved the output of `pixi global` subcommands
- Many bug fixes

#### Added

- Add timeouts by @Hofer-Julian in [#2311](https://github.com/prefix-dev/pixi/pull/2311)

#### Changed

- Global update should add new executables by @nichmor in [#2298](https://github.com/prefix-dev/pixi/pull/2298)

- Add `pixi global install --with` by @Hofer-Julian in [#2332](https://github.com/prefix-dev/pixi/pull/2332)

#### Documentation

- Document where `pixi-global.toml` can be found by @Hofer-Julian in [#2304](https://github.com/prefix-dev/pixi/pull/2304)

- Add ros noetic example by @ruben-arts in [#2271](https://github.com/prefix-dev/pixi/pull/2271)

- Add nichita and julian to CITATION.cff by @Hofer-Julian in [#2327](https://github.com/prefix-dev/pixi/pull/2327)

- Improve keyring documentation to use pixi global by @olivier-lacroix in [#2318](https://github.com/prefix-dev/pixi/pull/2318)

#### Fixed

- `pixi global upgrade-all` error message by @Hofer-Julian in [#2296](https://github.com/prefix-dev/pixi/pull/2296)

- Select correct run environment by @ruben-arts in [#2301](https://github.com/prefix-dev/pixi/pull/2301)

- Adapt channels to work with newest rattler-build version by @Hofer-Julian in [#2306](https://github.com/prefix-dev/pixi/pull/2306)

- Hide obsolete commands in help page of `pixi global` by @chawyehsu in [#2320](https://github.com/prefix-dev/pixi/pull/2320)

- Typecheck all tests by @Hofer-Julian in [#2328](https://github.com/prefix-dev/pixi/pull/2328)

#### Refactor

- Improve upload errors by @ruben-arts in [#2303](https://github.com/prefix-dev/pixi/pull/2303)

#### New Contributors
* @gerlero made their first contribution in [#2300](https://github.com/prefix-dev/pixi/pull/2300)

### [0.33.0] - 2024-10-16
#### ✨ Highlights

This is the first release with the new `pixi global` implementation. It's a full reimplementation of `pixi global` where it now uses a manifest file just like `pixi` projects. This way you can declare your environments and save them to a VCS.

It also brings features like, adding dependencies to a global environment, and exposing multiple binaries from the same environment that are not part of the main installed packages.

Test it out with:
```shell
# Normal feature
pixi global install ipython

# New features
pixi global install \
    --environment science \           # Defined the environment name
    --expose scipython=ipython \      # Expose binaries under custom names
    ipython scipy                     # Define multiple dependencies for one environment
```

This should result in a manifest in `$HOME/.pixi/manifests/pixi-global.toml`:
```toml
version = 1

[envs.ipython]
channels = ["conda-forge"]
dependencies = { ipython = "*" }
exposed = { ipython = "ipython", ipython3 = "ipython3" }

[envs.science]
channels = ["conda-forge"]
dependencies = { ipython = "*", scipy = "*" }
exposed = { scipython = "ipython" }
```

#### 📖 Documentation
Checkout the updated documentation on this new feature:
- Main documentation on this tag: https://pixi.sh/v0.33.0/
- Global CLI documentation: https://pixi.sh/v0.33.0/reference/cli/#global
- The implementation documentation: https://pixi.sh/v0.33.0/features/global_tools/
- The initial design proposal: https://pixi.sh/v0.33.0/design_proposals/pixi_global_manifest/

### [0.32.2] - 2024-10-16
#### ✨ Highlights

- `pixi self-update` will only work on the binaries from the GitHub releases, avoiding accidentally breaking the installation.
- We now support `gcs://` conda registries.
- No more broken PowerShell after using `pixi shell`.

#### Changed
- Add support for `gcs://` conda registries by @clement-chaneching in [#2263](https://github.com/prefix-dev/pixi/pull/2263)

#### Documentation
- Small fixes in tutorials/python.md by @carschandler in [#2252](https://github.com/prefix-dev/pixi/pull/2252)
- Update `pixi list` docs by @Hofer-Julian in [#2269](https://github.com/prefix-dev/pixi/pull/2269)

#### Fixed
- Bind ctrl c listener so that it doesn't interfere on powershell by @wolfv in [#2260](https://github.com/prefix-dev/pixi/pull/2260)
- Explicitly run default environment by @ruben-arts in [#2273](https://github.com/prefix-dev/pixi/pull/2273)
- Parse env name on adding by @ruben-arts in [#2279](https://github.com/prefix-dev/pixi/pull/2279)

#### Refactor
- Make self-update a compile time feature by @freundTech in [#2213](https://github.com/prefix-dev/pixi/pull/2213)


#### New Contributors
* @clement-chaneching made their first contribution in [#2263](https://github.com/prefix-dev/pixi/pull/2263)
* @freundTech made their first contribution in [#2213](https://github.com/prefix-dev/pixi/pull/2213)

### [0.32.1] - 2024-10-08
#### Fixes
- Bump Rust version to `1.81` by @wolfv in [#2227](https://github.com/prefix-dev/pixi/pull/2227)

#### Documentation
- Pixi-pack, docker, devcontainer by @pavelzw in [#2220](https://github.com/prefix-dev/pixi/pull/2220)

### [0.32.0] - 2024-10-08
#### ✨ Highlights

The biggest fix in this PR is the move to the latest rattler as it came with some major bug fixes for macOS and Rust 1.81 compatibility.

#### Changed
- Correctly implement total ordering for dependency provider by @tdejager in [rattler/#892](https://github.com/conda/rattler/pull/892)

#### Fixed
- Fixed self-clobber issue when up/down grading packages by @wolfv in [rattler/#893](https://github.com/conda/rattler/pull/893)
- Check environment name before returning not found print by @ruben-arts in [#2198](https://github.com/prefix-dev/pixi/pull/2198)
- Turn off symlink follow for task cache by @ruben-arts in [#2209](https://github.com/prefix-dev/pixi/pull/2209)


### [0.31.0] - 2024-10-03
#### ✨ Highlights
Thanks to our maintainer @baszamstra!
He sped up the resolver for all cases we could think of in [#2162](https://github.com/prefix-dev/pixi/pull/2162)
Check the result of times it takes to solve the environments in our test set:
![image](https://private-user-images.githubusercontent.com/4995967/371994129-0c89b07f-7e29-430a-b876-a8a5826bbc9d.png?jwt=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJnaXRodWIuY29tIiwiYXVkIjoicmF3LmdpdGh1YnVzZXJjb250ZW50LmNvbSIsImtleSI6ImtleTUiLCJleHAiOjE3Mjc5NjE2MzUsIm5iZiI6MTcyNzk2MTMzNSwicGF0aCI6Ii80OTk1OTY3LzM3MTk5NDEyOS0wYzg5YjA3Zi03ZTI5LTQzMGEtYjg3Ni1hOGE1ODI2YmJjOWQucG5nP1gtQW16LUFsZ29yaXRobT1BV1M0LUhNQUMtU0hBMjU2JlgtQW16LUNyZWRlbnRpYWw9QUtJQVZDT0RZTFNBNTNQUUs0WkElMkYyMDI0MTAwMyUyRnVzLWVhc3QtMSUyRnMzJTJGYXdzNF9yZXF1ZXN0JlgtQW16LURhdGU9MjAyNDEwMDNUMTMxNTM1WiZYLUFtei1FeHBpcmVzPTMwMCZYLUFtei1TaWduYXR1cmU9YjBlMTI5MmUxYWY5NmVkZmIwYmE5YTIwNTMyN2VkNDkwNjljZDE5ZjMzNzVkZTg4YWYyY2I2MjExZTAyNDY2NiZYLUFtei1TaWduZWRIZWFkZXJzPWhvc3QifQ.vh3Fs0MGdoPR0k-BmjGArXEekrlPV5N9wNM2CUq8e44)


#### Added

- Add `nodefaults` to imported conda envs by @ruben-arts in [#2097](https://github.com/prefix-dev/pixi/pull/2097)
- Add newline to `.gitignore` by @ruben-arts in [#2095](https://github.com/prefix-dev/pixi/pull/2095)
- Add `--no-activation` option to prevent env activation during global install/upgrade by @183amir in [#1980](https://github.com/prefix-dev/pixi/pull/1980)
- Add `--priority` arg to `project channel add` by @minrk in [#2086](https://github.com/prefix-dev/pixi/pull/2086)

#### Changed

- Use pixi spec for conda environment yml by @ruben-arts in [#2096](https://github.com/prefix-dev/pixi/pull/2096)
- Update rattler by @nichmor in [#2120](https://github.com/prefix-dev/pixi/pull/2120)
- Update README.md by @ruben-arts in [#2129](https://github.com/prefix-dev/pixi/pull/2129)
- Follow symlinks while walking files by @0xbe7a in [#2141](https://github.com/prefix-dev/pixi/pull/2141)

#### Documentation

- Adapt wording in pixi global proposal by @Hofer-Julian in [#2098](https://github.com/prefix-dev/pixi/pull/2098)
- Community: add array-api-extra by @lucascolley in [#2107](https://github.com/prefix-dev/pixi/pull/2107)
- `pixi global` mention `no-activation` by @Hofer-Julian in [#2109](https://github.com/prefix-dev/pixi/pull/2109)
- Add minimal constructor example by @bollwyvl in [#2102](https://github.com/prefix-dev/pixi/pull/2102)
- Update global manifest `install` by @Hofer-Julian in [#2128](https://github.com/prefix-dev/pixi/pull/2128)
- Add description for `pixi update --json` by @scottamain in [#2160](https://github.com/prefix-dev/pixi/pull/2160)
- Fixes backticks for doc strings by @rachfop in [#2174](https://github.com/prefix-dev/pixi/pull/2174)

#### Fixed

- Sort exported conda explicit spec topologically by @synapticarbors in [#2101](https://github.com/prefix-dev/pixi/pull/2101)
- `--import env_file` breaks channel priority by @fecet in [#2113](https://github.com/prefix-dev/pixi/pull/2113)
- Allow exact yanked pypi packages by @nichmor in [#2116](https://github.com/prefix-dev/pixi/pull/2116)
- Check if files are same in `self-update` by @apoorvkh in [#2132](https://github.com/prefix-dev/pixi/pull/2132)
- `get_or_insert_nested_table` by @Hofer-Julian in [#2167](https://github.com/prefix-dev/pixi/pull/2167)
- Improve `install.sh` PATH handling and general robustness by @Arcitec in [#2189](https://github.com/prefix-dev/pixi/pull/2189)
- Output tasks on `pixi run` without input by @ruben-arts in [#2193](https://github.com/prefix-dev/pixi/pull/2193)


#### Performance
- Significantly speed up conda resolution by @baszalmstra in [#2162](https://github.com/prefix-dev/pixi/pull/2162)


#### New Contributors
* @Arcitec made their first contribution in [#2189](https://github.com/prefix-dev/pixi/pull/2189)
* @rachfop made their first contribution in [#2174](https://github.com/prefix-dev/pixi/pull/2174)
* @scottamain made their first contribution in [#2160](https://github.com/prefix-dev/pixi/pull/2160)
* @apoorvkh made their first contribution in [#2132](https://github.com/prefix-dev/pixi/pull/2132)
* @0xbe7a made their first contribution in [#2141](https://github.com/prefix-dev/pixi/pull/2141)
* @fecet made their first contribution in [#2113](https://github.com/prefix-dev/pixi/pull/2113)
* @minrk made their first contribution in [#2086](https://github.com/prefix-dev/pixi/pull/2086)
* @183amir made their first contribution in [#1980](https://github.com/prefix-dev/pixi/pull/1980)
* @lucascolley made their first contribution in [#2107](https://github.com/prefix-dev/pixi/pull/2107)

### [0.30.0] - 2024-09-19
#### ✨ Highlights
I want to thank @synapticarbors and @abkfenris for starting the work on `pixi project export`.
Pixi now supports the export of a conda `environment.yml` file and a conda explicit specification file.
This is a great addition to the project and will help users to share their projects with other non pixi users.

#### Added
- Export conda explicit specification file from project by @synapticarbors in [#1873](https://github.com/prefix-dev/pixi/pull/1873)
- Add flag to `pixi search` by @Hofer-Julian in [#2018](https://github.com/prefix-dev/pixi/pull/2018)
- Adds the ability to set the index strategy by @tdejager in [#1986](https://github.com/prefix-dev/pixi/pull/1986)
- Export conda `environment.yml` by @abkfenris in [#2003](https://github.com/prefix-dev/pixi/pull/2003)

#### Changed
- Improve examples/docker by @jennydaman in [#1965](https://github.com/prefix-dev/pixi/pull/1965)
- Minimal pre-commit tasks by @Hofer-Julian in [#1984](https://github.com/prefix-dev/pixi/pull/1984)
- Improve error and feedback when target does not exist by @tdejager in [#1961](https://github.com/prefix-dev/pixi/pull/1961)
- Move the rectangle using a mouse in SDL by @certik in [#2069](https://github.com/prefix-dev/pixi/pull/2069)

#### Documentation
- Update cli.md by @xela-95 in [#2047](https://github.com/prefix-dev/pixi/pull/2047)
- Update `system-requirements` information by @ruben-arts in [#2079](https://github.com/prefix-dev/pixi/pull/2079)
- Append to file syntax in task docs by @nicornk in [#2013](https://github.com/prefix-dev/pixi/pull/2013)
- Change documentation of pixi upload to refer to correct API endpoint by @traversaro in [#2074](https://github.com/prefix-dev/pixi/pull/2074)

#### Testing
- Add downstream nerfstudio test by @tdejager in [#1996](https://github.com/prefix-dev/pixi/pull/1996)
- Run pytests in parallel by @tdejager in [#2027](https://github.com/prefix-dev/pixi/pull/2027)
- Testing common wheels by @tdejager in [#2031](https://github.com/prefix-dev/pixi/pull/2031)

#### Fixed
- Lock file is always outdated for pypi path dependencies by @nichmor in [#2039](https://github.com/prefix-dev/pixi/pull/2039)
- Fix error message for export conda explicit spec by @synapticarbors in [#2048](https://github.com/prefix-dev/pixi/pull/2048)
- Use `conda-pypi-map` for feature channels by @nichmor in [#2038](https://github.com/prefix-dev/pixi/pull/2038)
- Constrain feature platforms in schema by @bollwyvl in [#2055](https://github.com/prefix-dev/pixi/pull/2055)
- Split tag creation functions by @tdejager in [#2062](https://github.com/prefix-dev/pixi/pull/2062)
- Tree print to pipe by @ruben-arts in [#2064](https://github.com/prefix-dev/pixi/pull/2064)
- `subdirectory` in pypi url by @ruben-arts in [#2065](https://github.com/prefix-dev/pixi/pull/2065)
- Create a GUI application on Windows, not Console by @certik in [#2067](https://github.com/prefix-dev/pixi/pull/2067)
- Make dashes underscores in python package names by @ruben-arts in [#2073](https://github.com/prefix-dev/pixi/pull/2073)
- Give better errors on broken `pyproject.toml` by @ruben-arts in [#2075](https://github.com/prefix-dev/pixi/pull/2075)

#### Refactor
- Stop duplicating `strip_channel_alias` from rattler by @Hofer-Julian in [#2017](https://github.com/prefix-dev/pixi/pull/2017)
- Follow-up wheels tests by @Hofer-Julian in [#2063](https://github.com/prefix-dev/pixi/pull/2063)
- Integration test suite by @Hofer-Julian in [#2081](https://github.com/prefix-dev/pixi/pull/2081)
- Remove `psutils` by @Hofer-Julian in [#2083](https://github.com/prefix-dev/pixi/pull/2083)
- Add back older caching method by @tdejager in [#2046](https://github.com/prefix-dev/pixi/pull/2046)
- Release script by @Hofer-Julian in [#1978](https://github.com/prefix-dev/pixi/pull/1978)
- Activation script by @Hofer-Julian in [#2014](https://github.com/prefix-dev/pixi/pull/2014)
- Pins python version in add_pypi_functionality by @tdejager in [#2040](https://github.com/prefix-dev/pixi/pull/2040)
- Improve the lock_file_usage flags and behavior. by @ruben-arts in [#2078](https://github.com/prefix-dev/pixi/pull/2078)
- Move matrix to workflow that it is used in by @tdejager in [#1987](https://github.com/prefix-dev/pixi/pull/1987)
- Refactor manifest into more generic approach by @nichmor in [#2015](https://github.com/prefix-dev/pixi/pull/2015)

#### New Contributors
* @certik made their first contribution in [#2069](https://github.com/prefix-dev/pixi/pull/2069)
* @xela-95 made their first contribution in [#2047](https://github.com/prefix-dev/pixi/pull/2047)
* @nicornk made their first contribution in [#2013](https://github.com/prefix-dev/pixi/pull/2013)
* @jennydaman made their first contribution in [#1965](https://github.com/prefix-dev/pixi/pull/1965)

### [0.29.0] - 2024-09-04
#### ✨ Highlights

- Add build-isolation options, for more details check out our [docs](https://pixi.sh/v0.29.0/reference/project_configuration/#no-build-isolation)
- Allow to use virtual package overrides from environment variables ([PR](https://github.com/conda/rattler/pull/818))
- Many bug fixes


#### Added

- Add build-isolation options by @tdejager in [#1909](https://github.com/prefix-dev/pixi/pull/1909)


- Add release script by @Hofer-Julian in [#1971](https://github.com/prefix-dev/pixi/pull/1971)


#### Changed

- Use rustls-tls instead of native-tls per default by @Hofer-Julian in [#1929](https://github.com/prefix-dev/pixi/pull/1929)


- Upgrade to uv 0.3.4 by @tdejager in [#1936](https://github.com/prefix-dev/pixi/pull/1936)


- Upgrade to uv 0.4.0 by @tdejager in [#1944](https://github.com/prefix-dev/pixi/pull/1944)


- Better error for when the target or platform are missing by @tdejager in [#1959](https://github.com/prefix-dev/pixi/pull/1959)


- Improve integration tests by @Hofer-Julian in [#1958](https://github.com/prefix-dev/pixi/pull/1958)


- Improve release script by @Hofer-Julian in [#1974](https://github.com/prefix-dev/pixi/pull/1974)


#### Fixed

- Update env variables in installation docs by @lev112 in [#1937](https://github.com/prefix-dev/pixi/pull/1937)


- Always overwrite when pixi adding the dependency by @ruben-arts in [#1935](https://github.com/prefix-dev/pixi/pull/1935)


- Typo in schema.json by @SobhanMP in [#1948](https://github.com/prefix-dev/pixi/pull/1948)


- Using file url as mapping by @nichmor in [#1930](https://github.com/prefix-dev/pixi/pull/1930)


- Offline mapping should not request by @nichmor in [#1968](https://github.com/prefix-dev/pixi/pull/1968)


- `pixi init` for `pyproject.toml` by @Hofer-Julian in [#1947](https://github.com/prefix-dev/pixi/pull/1947)


- Use two in memory indexes, for resolve and builds by @tdejager in [#1969](https://github.com/prefix-dev/pixi/pull/1969)


- Minor issues and todos by @KGrewal1 in [#1963](https://github.com/prefix-dev/pixi/pull/1963)


#### Refactor

- Improve integration tests by @Hofer-Julian in [#1942](https://github.com/prefix-dev/pixi/pull/1942)


#### New Contributors
* @SobhanMP made their first contribution in [#1948](https://github.com/prefix-dev/pixi/pull/1948)
* @lev112 made their first contribution in [#1937](https://github.com/prefix-dev/pixi/pull/1937)

### [0.28.2] - 2024-08-28
#### Changed

- Use mold on linux by @Hofer-Julian in [#1914](https://github.com/prefix-dev/pixi/pull/1914)

#### Documentation

- Fix global manifest by @Hofer-Julian in [#1912](https://github.com/prefix-dev/pixi/pull/1912)
- Document azure keyring usage by @tdejager in [#1913](https://github.com/prefix-dev/pixi/pull/1913)

#### Fixed

- Let `init` add dependencies independent of target and don't install by @ruben-arts in [#1916](https://github.com/prefix-dev/pixi/pull/1916)
- Enable use of manylinux wheeltags once again by @tdejager in [#1925](https://github.com/prefix-dev/pixi/pull/1925)
- The bigger runner by @ruben-arts in [#1902](https://github.com/prefix-dev/pixi/pull/1902)

### [0.28.1] - 2024-08-26
#### Changed
- Uv upgrade to 0.3.2 by @tdejager in [#1900](https://github.com/prefix-dev/pixi/pull/1900)

#### Documentation
- Add `keyrings.artifacts` to the list of project built with `pixi` by @jslorrma in [#1908](https://github.com/prefix-dev/pixi/pull/1908)

#### Fixed
- Use default indexes if non where given by the lockfile by @ruben-arts in [#1910](https://github.com/prefix-dev/pixi/pull/1910)

#### New Contributors
* @jslorrma made their first contribution in [#1908](https://github.com/prefix-dev/pixi/pull/1908)

### [0.28.0] - 2024-08-22
#### ✨ Highlights
- **Bug Fixes**: Major fixes in general but especially for PyPI installation issues and better error messaging.
- **Compatibility**: Default Linux version downgraded to 4.18 for broader support.
- **New Features**: Added INIT_CWD in pixi run, improved logging, and more cache options.

#### Added

- Add `INIT_CWD` to activated env `pixi run` by @ruben-arts in [#1798](https://github.com/prefix-dev/pixi/pull/1798)
- Add context to error when parsing conda-meta files by @baszalmstra in [#1854](https://github.com/prefix-dev/pixi/pull/1854)
- Add some logging for when packages are actually overridden by conda by @tdejager in [#1874](https://github.com/prefix-dev/pixi/pull/1874)
- Add package when extra is added by @ruben-arts in [#1856](https://github.com/prefix-dev/pixi/pull/1856)

#### Changed

- Use new gateway to get the repodata for global install by @nichmor in [#1767](https://github.com/prefix-dev/pixi/pull/1767)
- Pixi global proposal by @Hofer-Julian in [#1757](https://github.com/prefix-dev/pixi/pull/1757)
- Upgrade to new uv 0.2.37 by @tdejager in [#1829](https://github.com/prefix-dev/pixi/pull/1829)
- Use new gateway for pixi search by @nichmor in [#1819](https://github.com/prefix-dev/pixi/pull/1819)
- Extend pixi clean cache with more cache options by @ruben-arts in [#1872](https://github.com/prefix-dev/pixi/pull/1872)
- Downgrade `__linux` default to `4.18` by @ruben-arts in [#1887](https://github.com/prefix-dev/pixi/pull/1887)

#### Documentation

- Fix instructions for update github actions by @Hofer-Julian in [#1774](https://github.com/prefix-dev/pixi/pull/1774)
- Fix fish completion script by @dennis-wey in [#1789](https://github.com/prefix-dev/pixi/pull/1789)
- Expands the environment variable examples in the reference section by @travishathaway in [#1779](https://github.com/prefix-dev/pixi/pull/1779)
- Community feedback `pixi global` by @Hofer-Julian in [#1800](https://github.com/prefix-dev/pixi/pull/1800)
- Additions to the pixi global proposal by @Hofer-Julian in [#1803](https://github.com/prefix-dev/pixi/pull/1803)
- Stop using invalid environment name in pixi global proposal by @Hofer-Julian in [#1826](https://github.com/prefix-dev/pixi/pull/1826)
- Extend `pixi global` proposal by @Hofer-Julian in [#1861](https://github.com/prefix-dev/pixi/pull/1861)
- Make `channels` required in `pixi global` manifest by @Hofer-Julian in [#1868](https://github.com/prefix-dev/pixi/pull/1868)
- Fix linux minimum version in project_configuration docs by @traversaro in [#1888](https://github.com/prefix-dev/pixi/pull/1888)

#### Fixed

- Try to increase `rlimit` by @baszalmstra in [#1766](https://github.com/prefix-dev/pixi/pull/1766)
- Add test for invalid environment names by @Hofer-Julian in [#1825](https://github.com/prefix-dev/pixi/pull/1825)
- Show global config in info command by @ruben-arts in [#1807](https://github.com/prefix-dev/pixi/pull/1807)
- Correct documentation of PIXI_ENVIRONMENT_PLATFORMS by @traversaro in [#1842](https://github.com/prefix-dev/pixi/pull/1842)
- Format in docs/features/environment.md by @cdeil in [#1846](https://github.com/prefix-dev/pixi/pull/1846)
- Make proper use of `NamedChannelOrUrl` by @ruben-arts in [#1820](https://github.com/prefix-dev/pixi/pull/1820)
- Trait impl override by @baszalmstra in [#1848](https://github.com/prefix-dev/pixi/pull/1848)
- Tame `pixi search` by @baszalmstra in [#1849](https://github.com/prefix-dev/pixi/pull/1849)
- Fix `pixi tree -i` duplicate output by @baszalmstra in [#1847](https://github.com/prefix-dev/pixi/pull/1847)
- Improve spec parsing error messages by @baszalmstra in [#1786](https://github.com/prefix-dev/pixi/pull/1786)
- Parse matchspec from CLI Lenient by @baszalmstra in [#1852](https://github.com/prefix-dev/pixi/pull/1852)
- Improve parsing of pypi-dependencies by @baszalmstra in [#1851](https://github.com/prefix-dev/pixi/pull/1851)
- Don't enforce system requirements for task tests by @baszalmstra in [#1855](https://github.com/prefix-dev/pixi/pull/1855)
- Satisfy when there are no pypi packages in the lockfile by @ruben-arts in [#1862](https://github.com/prefix-dev/pixi/pull/1862)
- Ssh url should not contain colon by @baszalmstra in [#1865](https://github.com/prefix-dev/pixi/pull/1865)
- `find-links` with manifest-path by @baszalmstra in [#1864](https://github.com/prefix-dev/pixi/pull/1864)
- Increase stack size in debug mode on windows by @baszalmstra in [#1867](https://github.com/prefix-dev/pixi/pull/1867)
- Solve-group-envs should reside in `.pixi` folder by @baszalmstra in [#1866](https://github.com/prefix-dev/pixi/pull/1866)
- Move package-override logging by @tdejager in [#1883](https://github.com/prefix-dev/pixi/pull/1883)
- Pinning logic for minor and major by @baszalmstra in [#1885](https://github.com/prefix-dev/pixi/pull/1885)
- Docs manifest tests by @ruben-arts in [#1879](https://github.com/prefix-dev/pixi/pull/1879)


#### Refactor
- Encapsulate channel resolution logic for CLI by @olivier-lacroix in [#1781](https://github.com/prefix-dev/pixi/pull/1781)
- Move to `pub(crate) fn` in order to detect and remove unused functions by @Hofer-Julian in [#1805](https://github.com/prefix-dev/pixi/pull/1805)
- Only compile `TaskNode::full_command` for tests by @Hofer-Julian in [#1809](https://github.com/prefix-dev/pixi/pull/1809)
- Derive `Default` for more structs by @Hofer-Julian in [#1824](https://github.com/prefix-dev/pixi/pull/1824)
- Rename `get_up_to_date_prefix` to `update_prefix` by @Hofer-Julian in [#1837](https://github.com/prefix-dev/pixi/pull/1837)
- Make `HasSpecs` implementation more functional by @Hofer-Julian in [#1863](https://github.com/prefix-dev/pixi/pull/1863)

#### New Contributors
* @cdeil made their first contribution in [#1846](https://github.com/prefix-dev/pixi/pull/1846)

### [0.27.1] - 2024-08-09
#### Documentation

- Fix mlx feature in "multiple machines" example by @rgommers in [#1762](https://github.com/prefix-dev/pixi/pull/1762)
- Update some of the cli and add osx rosetta mention by @ruben-arts in [#1760](https://github.com/prefix-dev/pixi/pull/1760)
- Fix typo by @pavelzw in [#1771](https://github.com/prefix-dev/pixi/pull/1771)

#### Fixed

- User agent string was wrong by @wolfv in [#1759](https://github.com/prefix-dev/pixi/pull/1759)
- Dont accidentally wipe pyproject.toml on `init` by @ruben-arts in [#1775](https://github.com/prefix-dev/pixi/pull/1775)

#### Refactor

- Add `pixi_spec` crate by @baszalmstra in [#1741](https://github.com/prefix-dev/pixi/pull/1741)

#### New Contributors
* @rgommers made their first contribution in [#1762](https://github.com/prefix-dev/pixi/pull/1762)

### [0.27.0] - 2024-08-07
#### ✨ Highlights

This release contains a lot of refactoring and improvements to the codebase, in preparation for future features and improvements.
Including with that we've fixed a ton of bugs. To make sure we're not breaking anything we've added a lot of tests and CI checks.
But let us know if you find any issues!

As a reminder, you can update pixi using `pixi self-update` and move to a specific version, including backwards, with `pixi self-update --version 0.27.0`.

#### Added

- Add `pixi run` completion for `fish` shell by @dennis-wey in [#1680](https://github.com/prefix-dev/pixi/pull/1680)

#### Changed

- Move examples from setuptools to hatchling by @Hofer-Julian in [#1692](https://github.com/prefix-dev/pixi/pull/1692)
- Let `pixi init` create hatchling pyproject.toml by @Hofer-Julian in [#1693](https://github.com/prefix-dev/pixi/pull/1693)
- Make `[project]` table optional for `pyproject.toml` manifests by @olivier-lacroix in [#1732](https://github.com/prefix-dev/pixi/pull/1732)

#### Documentation

- Improve the `fish` completions location by @tdejager in [#1647](https://github.com/prefix-dev/pixi/pull/1647)
- Explain why we use `hatchling` by @Hofer-Julian
- Update install CLI doc now that the `update` command exist by @olivier-lacroix in [#1690](https://github.com/prefix-dev/pixi/pull/1690)
- Mention `pixi exec` in GHA docs by @pavelzw in [#1724](https://github.com/prefix-dev/pixi/pull/1724)
- Update to correct spelling by @ahnsn in [#1730](https://github.com/prefix-dev/pixi/pull/1730)
- Ensure `hatchling` is used everywhere in documentation by @olivier-lacroix in [#1733](https://github.com/prefix-dev/pixi/pull/1733)
- Add readme to WASM example by @wolfv in [#1703](https://github.com/prefix-dev/pixi/pull/1703)
- Fix typo by @pavelzw in [#1660](https://github.com/prefix-dev/pixi/pull/1660)
- Fix typo by @DimitriPapadopoulos in [#1743](https://github.com/prefix-dev/pixi/pull/1743)
- Fix typo by @SeaOtocinclus in [#1651](https://github.com/prefix-dev/pixi/pull/1651)

#### Testing

- Added script and tasks for testing examples by @tdejager in [#1671](https://github.com/prefix-dev/pixi/pull/1671)
- Add simple integration tests by @ruben-arts in [#1719](https://github.com/prefix-dev/pixi/pull/1719)

#### Fixed

- Prepend pixi to path instead of appending by @vigneshmanick in [#1644](https://github.com/prefix-dev/pixi/pull/1644)
- Add manifest tests and run them in ci by @ruben-arts in [#1667](https://github.com/prefix-dev/pixi/pull/1667)
- Use hashed pypi mapping by @baszalmstra in [#1663](https://github.com/prefix-dev/pixi/pull/1663)
- Depend on `pep440_rs` from crates.io and use replace by @baszalmstra in [#1698](https://github.com/prefix-dev/pixi/pull/1698)
- `pixi add` with more than just package name and version by @ruben-arts in [#1704](https://github.com/prefix-dev/pixi/pull/1704)
- Ignore pypi logic on non pypi projects by @ruben-arts in [#1705](https://github.com/prefix-dev/pixi/pull/1705)
- Fix and refactor `--no-lockfile-update` by @ruben-arts in [#1683](https://github.com/prefix-dev/pixi/pull/1683)
- Changed example to use hatchling by @tdejager in [#1729](https://github.com/prefix-dev/pixi/pull/1729)
- Todo clean up by @KGrewal1 in [#1735](https://github.com/prefix-dev/pixi/pull/1735)
- Allow for init to `pixi.toml` when `pyproject.toml` is available. by @ruben-arts in [#1640](https://github.com/prefix-dev/pixi/pull/1640)
- Test on `macos-13` by @ruben-arts in [#1739](https://github.com/prefix-dev/pixi/pull/1739)
- Make sure pixi vars are available before `activation.env` vars are by @ruben-arts in [#1740](https://github.com/prefix-dev/pixi/pull/1740)
- Authenticate exec package download by @olivier-lacroix in [#1751](https://github.com/prefix-dev/pixi/pull/1751)

#### Refactor

- Extract `pixi_manifest` by @baszalmstra in [#1656](https://github.com/prefix-dev/pixi/pull/1656)
- Delay channel config url evaluation by @baszalmstra in [#1662](https://github.com/prefix-dev/pixi/pull/1662)
- Split out pty functionality by @tdejager in [#1678](https://github.com/prefix-dev/pixi/pull/1678)
- Make project manifest loading DRY and consistent by @olivier-lacroix in [#1688](https://github.com/prefix-dev/pixi/pull/1688)
- Refactor channel add and remove CLI commands by @olivier-lacroix in [#1689](https://github.com/prefix-dev/pixi/pull/1689)
- Refactor `pixi::consts` and `pixi::config` into separate crates by @tdejager in [#1684](https://github.com/prefix-dev/pixi/pull/1684)
- Move dependencies to `pixi_manifest` by @tdejager in [#1700](https://github.com/prefix-dev/pixi/pull/1700)
- Moved pypi environment modifiers by @tdejager in [#1699](https://github.com/prefix-dev/pixi/pull/1699)
- Split `HasFeatures` by @tdejager in [#1712](https://github.com/prefix-dev/pixi/pull/1712)
- Move, splits and renames the `HasFeatures` trait by @tdejager in [#1717](https://github.com/prefix-dev/pixi/pull/1717)
- Merge `utils` by @tdejager in [#1718](https://github.com/prefix-dev/pixi/pull/1718)
- Move `fancy` to its own crate by @tdejager in [#1722](https://github.com/prefix-dev/pixi/pull/1722)
- Move `config` to repodata functions by @tdejager in [#1723](https://github.com/prefix-dev/pixi/pull/1723)
- Move `pypi-mapping` to its own crate by @tdejager in [#1725](https://github.com/prefix-dev/pixi/pull/1725)
- Split `utils` into 2 crates by @tdejager in [#1736](https://github.com/prefix-dev/pixi/pull/1736)
- Add progress bar as a crate by @nichmor in [#1727](https://github.com/prefix-dev/pixi/pull/1727)
- Split up `pixi_manifest` lib by @tdejager in [#1661](https://github.com/prefix-dev/pixi/pull/1661)


#### New Contributors
* @DimitriPapadopoulos made their first contribution in [#1743](https://github.com/prefix-dev/pixi/pull/1743)
* @KGrewal1 made their first contribution in [#1735](https://github.com/prefix-dev/pixi/pull/1735)
* @ahnsn made their first contribution in [#1730](https://github.com/prefix-dev/pixi/pull/1730)
* @dennis-wey made their first contribution in [#1680](https://github.com/prefix-dev/pixi/pull/1680)

### [0.26.1] - 2024-07-22
#### Fixed
- Make sure we also build the msi installer by @ruben-arts in [#1645](https://github.com/prefix-dev/pixi/pull/1645)



### [0.26.0] - 2024-07-19
#### ✨ Highlights
- Specify how pixi **pins your dependencies** with the `pinning-strategy` in the config. e.g. `semver` -> `>=1.2.3,<2` and `no-pin` -> `*`)  [#1516](https://github.com/prefix-dev/pixi/pull/1516)
- Specify how pixi **solves multiple channels** with `channel-priority` in the manifest. [#1631](https://github.com/prefix-dev/pixi/pull/1631)
#### Added

- Add short options to config location flags by @ruben-arts in [#1586](https://github.com/prefix-dev/pixi/pull/1586)
- Add a file guard to indicate if an environment is being installed by @baszalmstra in [#1593](https://github.com/prefix-dev/pixi/pull/1593)
- Add `pinning-strategy` to the configuration by @ruben-arts in [#1516](https://github.com/prefix-dev/pixi/pull/1516)
- Add `channel-priority` to the manifest and solve by @ruben-arts in [#1631](https://github.com/prefix-dev/pixi/pull/1631)
- Add `nushell` completion by @Hofer-Julian in [#1599](https://github.com/prefix-dev/pixi/pull/1599)
- Add `nushell` completions for `pixi run` by @Hofer-Julian in [#1627](https://github.com/prefix-dev/pixi/pull/1627)
- Add completion for `pixi run --environment` for nushell by @Hofer-Julian in [#1636](https://github.com/prefix-dev/pixi/pull/1636)


#### Changed

- Upgrade uv 0.2.18 by @tdejager in [#1540](https://github.com/prefix-dev/pixi/pull/1540)
- Refactor `pyproject.toml` parser by @nichmor in [#1592](https://github.com/prefix-dev/pixi/pull/1592)
- Interactive warning for packages in `pixi global install` by @ruben-arts in [#1626](https://github.com/prefix-dev/pixi/pull/1626)

#### Documentation

- Add WASM example with JupyterLite by @wolfv in [#1623](https://github.com/prefix-dev/pixi/pull/1623)
- Added LLM example by @ytjhai in [#1545](https://github.com/prefix-dev/pixi/pull/1545)
- Add note to mark directory as excluded in pixi-pycharm by @pavelzw in [#1579](https://github.com/prefix-dev/pixi/pull/1579)
- Add changelog to docs by @vigneshmanick in [#1574](https://github.com/prefix-dev/pixi/pull/1574)
- Updated the values of the system requirements by @tdejager in [#1575](https://github.com/prefix-dev/pixi/pull/1575)
- Tell cargo install which bin to install by @ruben-arts in [#1584](https://github.com/prefix-dev/pixi/pull/1584)
- Update conflict docs for `cargo add` by @Hofer-Julian in [#1600](https://github.com/prefix-dev/pixi/pull/1600)
- Revert "Update conflict docs for `cargo add` " by @Hofer-Julian in [#1605](https://github.com/prefix-dev/pixi/pull/1605)
- Add reference documentation for the exec command by @baszalmstra in [#1587](https://github.com/prefix-dev/pixi/pull/1587)
- Add transitioning docs for `poetry` and `conda` by @ruben-arts in [#1624](https://github.com/prefix-dev/pixi/pull/1624)
- Add pixi-pack by @pavelzw in [#1629](https://github.com/prefix-dev/pixi/pull/1629)
- Use '-' instead of '_' for package name by @olivier-lacroix in [#1628](https://github.com/prefix-dev/pixi/pull/1628)


#### Fixed

- Flaky task test by @tdejager in [#1581](https://github.com/prefix-dev/pixi/pull/1581)
- Pass command line arguments verbatim by @baszalmstra in [#1582](https://github.com/prefix-dev/pixi/pull/1582)
- Run clippy on all targets by @Hofer-Julian in [#1588](https://github.com/prefix-dev/pixi/pull/1588)
- Pre-commit install pixi task by @Hofer-Julian in [#1590](https://github.com/prefix-dev/pixi/pull/1590)
- Add `clap_complete_nushell` to dependencies by @Hofer-Julian in [#1625](https://github.com/prefix-dev/pixi/pull/1625)
- Write to `stdout` for machine readable output by @Hofer-Julian in [#1639](https://github.com/prefix-dev/pixi/pull/1639)


#### Refactor

- Migrate to workspace by @baszalmstra in [#1597](https://github.com/prefix-dev/pixi/pull/1597)

#### Removed

- Remove double manifest warning by @tdejager in [#1580](https://github.com/prefix-dev/pixi/pull/1580)

#### New Contributors
* @ytjhai made their first contribution in [#1545](https://github.com/prefix-dev/pixi/pull/1545)

### [0.25.0] - 2024-07-05
#### ✨ Highlights
- `pixi exec` command, execute commands in temporary environments, useful for testing in short-lived sessions.
-  We've bumped the default system-requirements to higher defaults: glibc (2.17 -> 2.28), osx64 (10.15 -> 13.0), osx-arm64 (11.0 -> 13.0). Let us know if this causes any issues. To keep the previous values please use a `system-requirements` table, this is explained [here](https://pixi.sh/latest/reference/project_configuration/#the-system-requirements-table)

#### Changed

- Bump system requirements by @wolfv in [#1553](https://github.com/prefix-dev/pixi/pull/1553)
- Better error when exec is missing a cmd by @tdejager in [#1565](https://github.com/prefix-dev/pixi/pull/1565)
- Make exec use authenticated client by @tdejager in [#1568](https://github.com/prefix-dev/pixi/pull/1568)

#### Documentation

- Automatic updating using github actions by @pavelzw in [#1456](https://github.com/prefix-dev/pixi/pull/1456)
- Describe the --change-ps1 option for pixi shell by @Yura52 in [#1536](https://github.com/prefix-dev/pixi/pull/1536)
- Add some other quantco repos by @pavelzw in [#1542](https://github.com/prefix-dev/pixi/pull/1542)
- Add example using `geos-rs` by @Hofer-Julian in [#1563](https://github.com/prefix-dev/pixi/pull/1563)

#### Fixed

- Tiny error in basic_usage.md by @Sjouks in [#1513](https://github.com/prefix-dev/pixi/pull/1513)
- Lazy initialize client by @baszalmstra in [#1511](https://github.com/prefix-dev/pixi/pull/1511)
- URL typos in rtd examples by @kklein in [#1538](https://github.com/prefix-dev/pixi/pull/1538)
- Fix satisfiability for short sha hashes by @tdejager in [#1530](https://github.com/prefix-dev/pixi/pull/1530)
- Wrong path passed to dynamic check by @tdejager in [#1552](https://github.com/prefix-dev/pixi/pull/1552)
- Don't error if no tasks is available on platform by @hoxbro in [#1550](https://github.com/prefix-dev/pixi/pull/1550)

#### Refactor

- Add to use update code by @baszalmstra in [#1508](https://github.com/prefix-dev/pixi/pull/1508)

#### New Contributors
* @kklein made their first contribution in [#1538](https://github.com/prefix-dev/pixi/pull/1538)
* @Yura52 made their first contribution in [#1536](https://github.com/prefix-dev/pixi/pull/1536)
* @Sjouks made their first contribution in [#1513](https://github.com/prefix-dev/pixi/pull/1513)

### [0.24.2] - 2024-06-14
#### Documentation
- Add readthedocs examples  by @bollwyvl in [#1423](https://github.com/prefix-dev/pixi/pull/1423)
- Fix typo in  project_configuration.md  by @RaulPL in [#1502](https://github.com/prefix-dev/pixi/pull/1502)

#### Fixed
- Too much shell variables in activation of `pixi shell`  by @ruben-arts in [#1507](https://github.com/prefix-dev/pixi/pull/1507)

### [0.24.1] - 2024-06-12

#### Fixed
- Replace http code %2b with + by @ruben-arts in [#1500](https://github.com/prefix-dev/pixi/pull/1500)

### [0.24.0] - 2024-06-12
#### ✨ Highlights

- You can now run in a more isolated environment on `unix` machines, using `pixi run --clean-env TASK_NAME`.
- You can new easily clean your environment with `pixi clean` or the cache with `pixi clean cache`

#### Added

- Add `pixi clean` command by @ruben-arts in [#1325](https://github.com/prefix-dev/pixi/pull/1325)
- Add `--clean-env` flag to tasks and run command by @ruben-arts in [#1395](https://github.com/prefix-dev/pixi/pull/1395)
- Add `description` field to `task` by @jjjermiah in [#1479](https://github.com/prefix-dev/pixi/pull/1479)
- Add pixi file to the environment to add pixi specific details by @ruben-arts in [#1495](https://github.com/prefix-dev/pixi/pull/1495)

#### Changed

- Project environment cli by @baszalmstra in [#1433](https://github.com/prefix-dev/pixi/pull/1433)
- Update task list console output by @vigneshmanick in [#1443](https://github.com/prefix-dev/pixi/pull/1443)
- Upgrade uv by @tdejager in [#1436](https://github.com/prefix-dev/pixi/pull/1436)
- Sort packages in `list_global_packages` by @dhirschfeld in [#1458](https://github.com/prefix-dev/pixi/pull/1458)
- Added test for special chars wheel filename by @tdejager in [#1454](https://github.com/prefix-dev/pixi/pull/1454)

#### Documentation
- Improve multi env tasks documentation by @ruben-arts in [#1494](https://github.com/prefix-dev/pixi/pull/1494)

#### Fixed
- Use the activated environment when running a task by @tdejager in [#1461](https://github.com/prefix-dev/pixi/pull/1461)
- Fix authentication pypi-deps for download from lockfile by @tdejager in [#1460](https://github.com/prefix-dev/pixi/pull/1460)
- Display channels correctly in `pixi info` by @ruben-arts in [#1459](https://github.com/prefix-dev/pixi/pull/1459)
- Render help for `--frozen` by @ruben-arts in [#1468](https://github.com/prefix-dev/pixi/pull/1468)
- Don't record purl for non conda-forge channels by @nichmor in [#1451](https://github.com/prefix-dev/pixi/pull/1451)
- Use best_platform to verify the run platform by @ruben-arts in [#1472](https://github.com/prefix-dev/pixi/pull/1472)
- Creation of parent dir of symlink by @ruben-arts in [#1483](https://github.com/prefix-dev/pixi/pull/1483)
- `pixi install --all` output missing newline by @vigneshmanick in [#1487](https://github.com/prefix-dev/pixi/pull/1487)
- Don't error on already existing dependency by @ruben-arts in [#1449](https://github.com/prefix-dev/pixi/pull/1449)
- Remove debug true in release by @ruben-arts in [#1477](https://github.com/prefix-dev/pixi/pull/1477)

#### New Contributors
* @dhirschfeld made their first contribution in [#1458](https://github.com/prefix-dev/pixi/pull/1458)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.23.0..HEAD)

### [0.23.0] - 2024-05-27

#### ✨ Highlights
- This release adds two new commands `pixi config` and `pixi update`
  - `pixi config` allows you to `edit`, `set`, `unset`, `append`, `prepend` and `list` your local/global or system configuration.
  - `pixi update` re-solves the full lockfile or use `pixi update PACKAGE` to only update `PACKAGE`, making sure your project is using the latest versions that the manifest allows for.

#### Added
- Add `pixi config` command by @chawyehsu in [#1339](https://github.com/prefix-dev/pixi/pull/1339)
- Add `pixi list --explicit` flag command by @jjjermiah in [#1403](https://github.com/prefix-dev/pixi/pull/1403)
- Add `[activation.env]` table for environment variables by @ruben-arts in [#1156](https://github.com/prefix-dev/pixi/pull/1156)
- Allow installing multiple envs, including `--all` at once by @tdejager in [#1413](https://github.com/prefix-dev/pixi/pull/1413)
- Add `pixi update` command to re-solve the lockfile by @baszalmstra in [#1431](https://github.com/prefix-dev/pixi/pull/1431) (fixes 20 :thumbsup:)
- Add `detached-environments` to the config, move environments outside the project folder by @ruben-arts in [#1381](https://github.com/prefix-dev/pixi/pull/1381) (fixes 11 :thumbsup:)

#### Changed
- Use the gateway to fetch repodata by @baszalmstra in [#1307](https://github.com/prefix-dev/pixi/pull/1307)
- Switch to compressed mapping by @nichmor in [#1335](https://github.com/prefix-dev/pixi/pull/1335)
- Warn on pypi conda clobbering by @nichmor in [#1353](https://github.com/prefix-dev/pixi/pull/1353)
- Align `remove` arguments with `add` by @olivier-lacroix in [#1406](https://github.com/prefix-dev/pixi/pull/1406)
- Add backward compat logic for older lock files by @nichmor in [#1425](https://github.com/prefix-dev/pixi/pull/1425)

#### Documentation
- Fix small screen by removing getting started section. by @ruben-arts in [#1393](https://github.com/prefix-dev/pixi/pull/1393)
- Improve caching docs by @ruben-arts in [#1422](https://github.com/prefix-dev/pixi/pull/1422)
- Add example, python library using gcp upload by @tdejager in [#1380](https://github.com/prefix-dev/pixi/pull/1380)
- Correct typos with `--no-lockfile-update`. by @tobiasraabe in [#1396](https://github.com/prefix-dev/pixi/pull/1396)

#### Fixed
- Trim channel url when filter packages_for_prefix_mapping by @zen-xu in [#1391](https://github.com/prefix-dev/pixi/pull/1391)
- Use the right channels when upgrading global packages by @olivier-lacroix in [#1326](https://github.com/prefix-dev/pixi/pull/1326)
- Fish prompt display looks wrong in tide by @tfriedel in [#1424](https://github.com/prefix-dev/pixi/pull/1424)
- Use local mapping instead of remote by @nichmor in [#1430](https://github.com/prefix-dev/pixi/pull/1430)

#### Refactor
- Remove unused fetch_sparse_repodata by @olivier-lacroix in [#1411](https://github.com/prefix-dev/pixi/pull/1411)
- Remove project level method that are per environment by @olivier-lacroix in [#1412](https://github.com/prefix-dev/pixi/pull/1412)
- Update lockfile functionality for reusability by @baszalmstra in [#1426](https://github.com/prefix-dev/pixi/pull/1426)

#### New Contributors
* @tfriedel made their first contribution in [#1424](https://github.com/prefix-dev/pixi/pull/1424)
* @jjjermiah made their first contribution in [#1403](https://github.com/prefix-dev/pixi/pull/1403)
* @tobiasraabe made their first contribution in [#1396](https://github.com/prefix-dev/pixi/pull/1396)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.22.0..v0.23.0)

### [0.22.0] - 2024-05-13
#### ✨ Highlights

- Support for source pypi dependencies through the cli:
  - `pixi add --pypi 'package @ package.whl'`, perfect for adding just build wheels to your environment in CI.
  - `pixi add --pypi 'package_from_git @ git+https://github.com/org/package.git'`, to add a package from a git repository.
  - `pixi add --pypi 'package_from_path @ file:///path/to/package' --editable`, to add a package from a local path.

#### Added
- Implement more functions for `pixi add --pypi` by @wolfv in [#1244](https://github.com/prefix-dev/pixi/pull/1244)

#### Documentation
- Update `install` cli doc by @vigneshmanick in [#1336](https://github.com/prefix-dev/pixi/pull/1336)
- Replace empty default example with no-default-feature by @beenje in [#1352](https://github.com/prefix-dev/pixi/pull/1352)
- Document the add & remove cli behaviour with pyproject.toml manifest by @olivier-lacroix in [#1338](https://github.com/prefix-dev/pixi/pull/1338)
- Add environment activation to GitHub actions docs by @pavelzw in [#1371](https://github.com/prefix-dev/pixi/pull/1371)
- Clarify in CLI that run can also take commands by @twrightsman in [#1368](https://github.com/prefix-dev/pixi/pull/1368)

#### Fixed

- Automated update of install script in pixi.sh by @ruben-arts in [#1351](https://github.com/prefix-dev/pixi/pull/1351)
- Wrong description on `pixi project help` by @notPlancha in [#1358](https://github.com/prefix-dev/pixi/pull/1358)
- Don't need a python interpreter when not having `pypi` dependencies. by @ruben-arts in [#1366](https://github.com/prefix-dev/pixi/pull/1366)
- Don't error on not editable not path by @ruben-arts in [#1365](https://github.com/prefix-dev/pixi/pull/1365)
- Align shell-hook cli with shell by @ruben-arts in [#1364](https://github.com/prefix-dev/pixi/pull/1364)
- Only write prefix file if needed by @ruben-arts in [#1363](https://github.com/prefix-dev/pixi/pull/1363)


#### Refactor
- Lock-file resolve functionality in separated modules by @tdejager in [#1337](https://github.com/prefix-dev/pixi/pull/1337)
- Use generic for RepoDataRecordsByName and PypiRecordsByName by @olivier-lacroix in [#1341](https://github.com/prefix-dev/pixi/pull/1341)


#### New Contributors
* @twrightsman made their first contribution in [#1368](https://github.com/prefix-dev/pixi/pull/1368)
* @notPlancha made their first contribution in [#1358](https://github.com/prefix-dev/pixi/pull/1358)
* @vigneshmanick made their first contribution in [#1336](https://github.com/prefix-dev/pixi/pull/1336)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.21.1..v0.22.0)


### [0.21.1] - 2024-05-07

#### Fixed
- Use read timeout, not global timeout by @wolfv in [#1329](https://github.com/prefix-dev/pixi/pull/1329)
- Channel priority logic by @ruben-arts in [#1332](https://github.com/prefix-dev/pixi/pull/1332)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.21.0..v0.21.1)

### [0.21.0] - 2024-05-06
#### ✨ Highlights
- This release adds support for configuring PyPI settings globally, to use alternative PyPI indexes and load credentials with keyring.
- We now support cross-platform running, for `osx-64` on `osx-arm64` and `wasm` environments.
- There is now a `no-default-feature` option to simplify usage of environments.

#### Added
- Add pypi config for global local config file + keyring support by @wolfv in [#1279](https://github.com/prefix-dev/pixi/pull/1279)
- Allow for cross-platform running, for `osx-64` on `osx-arm64` and `wasm` environments by @wolfv in [#1020](https://github.com/prefix-dev/pixi/pull/1020)

#### Changed
- Add `no-default-feature` option to environments by @olivier-lacroix in [#1092](https://github.com/prefix-dev/pixi/pull/1092)
- Add `/etc/pixi/config.toml` to global configuration search paths by @pavelzw in [#1304](https://github.com/prefix-dev/pixi/pull/1304)
- Change global config fields to kebab-case by @tdejager in [#1308](https://github.com/prefix-dev/pixi/pull/1308)
- Show all available task with `task list` by @Hoxbro in [#1286](https://github.com/prefix-dev/pixi/pull/1286)
- Allow to emit activation environment variables as JSON by @borchero in [#1317](https://github.com/prefix-dev/pixi/pull/1317)
- Use locked pypi packages as preferences in the pypi solve to get minimally updating lock files by @ruben-arts in [#1320](https://github.com/prefix-dev/pixi/pull/1320)
- Allow to upgrade several global packages at once by @olivier-lacroix in [#1324](https://github.com/prefix-dev/pixi/pull/1324)

#### Documentation
- Typo in tutorials python by @carschandler in [#1297](https://github.com/prefix-dev/pixi/pull/1297)
- Python Tutorial: Dependencies, PyPI, Order, Grammar by @JesperDramsch in [#1313](https://github.com/prefix-dev/pixi/pull/1313)

#### Fixed
- Schema version and add it to tbump by @ruben-arts in [#1284](https://github.com/prefix-dev/pixi/pull/1284)
- Make integration test fail in ci and fix ssh issue by @ruben-arts in [#1301](https://github.com/prefix-dev/pixi/pull/1301)
- Automate adding install scripts to the docs by @ruben-arts in [#1302](https://github.com/prefix-dev/pixi/pull/1302)
- Do not always request for prefix mapping by @nichmor in [#1300](https://github.com/prefix-dev/pixi/pull/1300)
- Align CLI aliases and add missing by @ruben-arts in [#1316](https://github.com/prefix-dev/pixi/pull/1316)
- Alias `depends_on` to `depends-on` by @ruben-arts in [#1310](https://github.com/prefix-dev/pixi/pull/1310)
- Add error if channel or platform doesn't exist on remove by @ruben-arts in [#1315](https://github.com/prefix-dev/pixi/pull/1315)
- Allow spec in `pixi q` instead of only name by @ruben-arts in [#1314](https://github.com/prefix-dev/pixi/pull/1314)
- Remove dependency on sysroot for linux by @ruben-arts in [#1319](https://github.com/prefix-dev/pixi/pull/1319)
- Fix linking symlink issue, by updating to the latest `rattler` by @baszalmstra in [#1327](https://github.com/prefix-dev/pixi/pull/1327)

#### Refactor
- Use IndexSet instead of Vec for collections of unique elements by @olivier-lacroix in [#1289](https://github.com/prefix-dev/pixi/pull/1289)
- Use generics over PyPiDependencies and CondaDependencies by @olivier-lacroix in [#1303](https://github.com/prefix-dev/pixi/pull/1303)

#### New Contributors
* @borchero made their first contribution in [#1317](https://github.com/prefix-dev/pixi/pull/1317)
* @JesperDramsch made their first contribution in [#1313](https://github.com/prefix-dev/pixi/pull/1313)
* @Hoxbro made their first contribution in [#1286](https://github.com/prefix-dev/pixi/pull/1286)
* @carschandler made their first contribution in [#1297](https://github.com/prefix-dev/pixi/pull/1297)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.20.1..v0.21.0)

### [0.20.1] - 2024-04-26
#### ✨ Highlights
- Big improvements on the pypi-editable installs.


#### Fixed
- Editable non-satisfiable by @baszalmstra in [#1251](https://github.com/prefix-dev/pixi/pull/1251)
- Satisfiability with pypi extras by @baszalmstra in [#1253](https://github.com/prefix-dev/pixi/pull/1253)
- Change global install activation script permission from 0o744 -> 0o755 by @zen-xu in [#1250](https://github.com/prefix-dev/pixi/pull/1250)
- Avoid creating Empty TOML tables by @olivier-lacroix in [#1270](https://github.com/prefix-dev/pixi/pull/1270)
- Uses the special-case uv path handling for both built and source by @tdejager in [#1263](https://github.com/prefix-dev/pixi/pull/1263)
- Modify test before attempting to write to .bash_profile in install.sh by @bruchim-cisco in [#1267](https://github.com/prefix-dev/pixi/pull/1267)
- Parse properly 'default' as environment Cli argument by @olivier-lacroix in [#1247](https://github.com/prefix-dev/pixi/pull/1247)
- Apply `schema.json` normalization, add to docs by @bollwyvl in [#1265](https://github.com/prefix-dev/pixi/pull/1265)
- Improve absolute path satisfiability by @tdejager in [#1252](https://github.com/prefix-dev/pixi/pull/1252)
- Improve parse deno error and make task a required field in the cli by @ruben-arts in [#1260](https://github.com/prefix-dev/pixi/pull/1260)

#### New Contributors
* @bollwyvl made their first contribution in [#1265](https://github.com/prefix-dev/pixi/pull/1265)
* @bruchim-cisco made their first contribution in [#1267](https://github.com/prefix-dev/pixi/pull/1267)
* @zen-xu made their first contribution in [#1250](https://github.com/prefix-dev/pixi/pull/1250)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.20.0..v0.20.1)


### [0.20.0] - 2024-04-19
#### ✨ Highlights

- We now support `env` variables in the `task` definition, these can also be used as default values for parameters in your task which you can overwrite with your shell's env variables.
e.g. `task = { cmd = "task to run", env = { VAR="value1", PATH="my/path:$PATH" } }`
- We made a big effort on fixing issues and improving documentation!

#### Added
- Add `env` to the tasks to specify tasks specific environment variables by @wolfv in https://github.com/prefix-dev/pixi/pull/972

#### Changed
- Add `--pyproject` option to `pixi init` with a pyproject.toml by @olivier-lacroix in [#1188](https://github.com/prefix-dev/pixi/pull/1188)
- Upgrade to new uv version 0.1.32 by @tdejager in [#1208](https://github.com/prefix-dev/pixi/pull/1208)

#### Documentation
- Document `pixi.lock` by @ruben-arts in [#1209](https://github.com/prefix-dev/pixi/pull/1209)
- Document channel `priority` definition by @ruben-arts in [#1234](https://github.com/prefix-dev/pixi/pull/1234)
- Add rust tutorial including openssl example by @ruben-arts in [#1155](https://github.com/prefix-dev/pixi/pull/1155)
- Add python tutorial to documentation by @tdejager in [#1179](https://github.com/prefix-dev/pixi/pull/1179)
- Add JupyterLab integration docs by @renan-r-santos in [#1147](https://github.com/prefix-dev/pixi/pull/1147)

- Add Windows support for PyCharm integration by @pavelzw in [#1192](https://github.com/prefix-dev/pixi/pull/1192)
- Setup_pixi for local pixi installation by @ytausch in [#1181](https://github.com/prefix-dev/pixi/pull/1181)
- Update pypi docs by @Hofer-Julian in [#1215](https://github.com/prefix-dev/pixi/pull/1215)
- Fix order of `--no-deps` when pip installing in editable mode by @glemaitre in [#1220](https://github.com/prefix-dev/pixi/pull/1220)
- Fix frozen documentation by @ruben-arts in [#1167](https://github.com/prefix-dev/pixi/pull/1167)

#### Fixed
- Small typo in list cli by @tdejager in [#1169](https://github.com/prefix-dev/pixi/pull/1169)
- Issue with invalid solve group by @baszalmstra in [#1190](https://github.com/prefix-dev/pixi/pull/1190)
- Improve error on parsing lockfile by @ruben-arts in [#1180](https://github.com/prefix-dev/pixi/pull/1180)
- Replace `_` with `-` when creating environments from features by @wolfv in [#1203](https://github.com/prefix-dev/pixi/pull/1203)
- Prevent duplicate direct dependencies in tree by @abkfenris in [#1184](https://github.com/prefix-dev/pixi/pull/1184)
- Use project root directory instead of task.working_directory for base dir when hashing by @wolfv in [#1202](https://github.com/prefix-dev/pixi/pull/1202)
- Do not leak env vars from bat scripts in cmd.exe by @wolfv in [#1205](https://github.com/prefix-dev/pixi/pull/1205)
- Make file globbing behave more as expected by @wolfv in [#1204](https://github.com/prefix-dev/pixi/pull/1204)
- Fix for using file::// in pyproject.toml dependencies by @tdejager in [#1196](https://github.com/prefix-dev/pixi/pull/1196)
- Improve pypi version conversion in pyproject.toml dependencies by @wolfv in [#1201](https://github.com/prefix-dev/pixi/pull/1201)
- Update to the latest rattler by @wolfv in [#1235](https://github.com/prefix-dev/pixi/pull/1235)

#### **BREAKING**
- `task = { cmd = "task to run", cwd = "folder", inputs = "input.txt", output = "output.txt"}` Where `input.txt` and `output.txt` where previously in `folder` they are now relative the project root. This changed in: [#1202](https://github.com/prefix-dev/pixi/pull/1202)
- `task = { cmd = "task to run", inputs = "input.txt"}` previously searched for all `input.txt` files now only for the ones in the project root. This changed in:  [#1204](https://github.com/prefix-dev/pixi/pull/1204)

#### New Contributors
* @glemaitre made their first contribution in [#1220](https://github.com/prefix-dev/pixi/pull/1220)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.19.1..v0.20.0)


### [0.19.1] - 2024-04-11
#### ✨ Highlights
This fixes the issue where pixi would generate broken environments/lockfiles when a mapping for a brand-new version of a package is missing.

#### Changed
- Add fallback mechanism for missing mapping by @nichmor in [#1166](https://github.com/prefix-dev/pixi/pull/1166)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.19.0..v0.19.1)

### [0.19.0] - 2024-04-10
#### ✨ Highlights
- This release adds a new `pixi tree` command to show the dependency tree of the project.
- Pixi now persists the manifest and environment when activating a shell, so you can use pixi as if you are in that folder while in the shell.

#### Added
- `pixi tree` command to show dependency tree by @abkfenris in [#1069](https://github.com/prefix-dev/pixi/pull/1069)
- Persistent shell manifests by @abkfenris in [#1080](https://github.com/prefix-dev/pixi/pull/1080)
- Add to pypi in feature (`pixi add --feature test --pypi package`) by @ruben-arts in [#1135](https://github.com/prefix-dev/pixi/pull/1135)
- Use new mapping by @nichmor in [#888](https://github.com/prefix-dev/pixi/pull/888)
- `--no-progress` to disable all progress bars by @baszalmstra in [#1105](https://github.com/prefix-dev/pixi/pull/1105)
- Create a table if channel is specified (`pixi add conda-forge::rattler-build`) by @baszalmstra in [#1079](https://github.com/prefix-dev/pixi/pull/1079)

#### Changed
- Add the project itself as an editable dependency by @olivier-lacroix in [#1084](https://github.com/prefix-dev/pixi/pull/1084)
- Get `tool.pixi.project.name` from `project.name` by @olivier-lacroix in [#1112](https://github.com/prefix-dev/pixi/pull/1112)
- Create `features` and `environments` from extras by @olivier-lacroix in [#1077](https://github.com/prefix-dev/pixi/pull/1077)
- Pypi supports come out of Beta by @olivier-lacroix in [#1120](https://github.com/prefix-dev/pixi/pull/1120)
- Enable to force `PIXI_ARCH` for pixi installation by @beenje in [#1129](https://github.com/prefix-dev/pixi/pull/1129)
- Improve tool.pixi.project detection logic by @olivier-lacroix in [#1127](https://github.com/prefix-dev/pixi/pull/1127)
- Add purls for packages if adding pypi dependencies by @nichmor in [#1148](https://github.com/prefix-dev/pixi/pull/1148)
- Add env name if not default to `tree` and `list` commands by @ruben-arts in [#1145](https://github.com/prefix-dev/pixi/pull/1145)

#### Documentation
- Add MODFLOW 6 to community docs by @Hofer-Julian in [#1125](https://github.com/prefix-dev/pixi/pull/1125)
- Addition of ros2 tutorial by @ruben-arts in [#1116](https://github.com/prefix-dev/pixi/pull/1116)
- Improve install script docs by @ruben-arts in [#1136](https://github.com/prefix-dev/pixi/pull/1136)
- More structured table of content by @tdejager in [#1142](https://github.com/prefix-dev/pixi/pull/1142)

#### Fixed
- Amend syntax in `conda-meta/history` to prevent `conda.history.History.parse()` error by @jaimergp in [#1117](https://github.com/prefix-dev/pixi/pull/1117)
- Fix docker example and include `pyproject.toml` by @tdejager in [#1121](https://github.com/prefix-dev/pixi/pull/1121)

#### New Contributors
* @abkfenris made their first contribution in [#1069](https://github.com/prefix-dev/pixi/pull/1069)
* @beenje made their first contribution in [#1129](https://github.com/prefix-dev/pixi/pull/1129)
* @jaimergp made their first contribution in [#1117](https://github.com/prefix-dev/pixi/pull/1117)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.18.0..v0.19.0)


### [0.18.0] - 2024-04-02
#### ✨ Highlights
- This release adds support for `pyproject.toml`, now pixi reads from the `[tool.pixi]` table.
- We now support editable PyPI dependencies, and PyPI source dependencies, including `git`, `path`, and `url` dependencies.

> [!TIP]
> These new features are part of the ongoing effort to make pixi more flexible, powerful, and comfortable for the python users.
> They are still in progress so expect more improvements on these features soon, so please report any issues you encounter and follow our next releases!

#### Added
- Support for `pyproject.toml` by @olivier-lacroix in [#999](https://github.com/prefix-dev/pixi/pull/999)
- Support for PyPI source dependencies by @tdejager in [#985](https://github.com/prefix-dev/pixi/pull/985)
- Support for editable PyPI dependencies by @tdejager in [#1044](https://github.com/prefix-dev/pixi/pull/1044)

#### Changed
- `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` compliance by @chawyehsu in [#1050](https://github.com/prefix-dev/pixi/pull/1050)
- Build pixi for windows arm by @baszalmstra in [#1053](https://github.com/prefix-dev/pixi/pull/1053)
- Platform literals by @baszalmstra in [#1054](https://github.com/prefix-dev/pixi/pull/1054)
- Cli docs: --user is actually --username
- Fixed error in auth example (CLI docs) by @ytausch in [#1076](https://github.com/prefix-dev/pixi/pull/1076)

#### Documentation
- Add lockfile update description in preparation for pixi update by @ruben-arts in [#1073](https://github.com/prefix-dev/pixi/pull/1073)
- `zsh` may be used for installation on macOS by @pya in [#1091](https://github.com/prefix-dev/pixi/pull/1091)
- Fix typo in `pixi auth` documentation by @ytausch in [#1076](https://github.com/prefix-dev/pixi/pull/1076)
- Add `rstudio` to the IDE integration docs by @wolfv in [#1144](https://github.com/prefix-dev/pixi/pull/1144)

#### Fixed
- Test failure on riscv64 by @hack3ric in [#1045](https://github.com/prefix-dev/pixi/pull/1045)
- Validation test was testing on a wrong pixi.toml by @ruben-arts in [#1056](https://github.com/prefix-dev/pixi/pull/1056)
- Pixi list shows path and editable by @baszalmstra in [#1100](https://github.com/prefix-dev/pixi/pull/1100)
- Docs ci by @ruben-arts in [#1074](https://github.com/prefix-dev/pixi/pull/1074)
- Add error for unsupported pypi dependencies by @baszalmstra in [#1052](https://github.com/prefix-dev/pixi/pull/1052)
- Interactively delete environment when it was relocated by @baszalmstra in [#1102](https://github.com/prefix-dev/pixi/pull/1102)
- Allow solving for different platforms by @baszalmstra in [#1101](https://github.com/prefix-dev/pixi/pull/1101)
- Don't allow extra keys in pypi requirements by @baszalmstra in [#1104](https://github.com/prefix-dev/pixi/pull/1104)
- Solve when moving dependency from conda to pypi by @baszalmstra in [#1099](https://github.com/prefix-dev/pixi/pull/1099)

#### New Contributors
* @pya made their first contribution in [#1091](https://github.com/prefix-dev/pixi/pull/1091)
* @ytausch made their first contribution in [#1076](https://github.com/prefix-dev/pixi/pull/1076)
* @hack3ric made their first contribution in [#1045](https://github.com/prefix-dev/pixi/pull/1045)
* @olivier-lacroix made their first contribution in [#999](https://github.com/prefix-dev/pixi/pull/999)
* @henryiii made their first contribution in [#1063](https://github.com/prefix-dev/pixi/pull/1063)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.17.1..v0.18.0)

### [0.17.1] - 2024-03-21
#### ✨ Highlights

A quick bug-fix release for `pixi list`.

#### Documentation

- Fix typo by @pavelzw in [#1028](https://github.com/prefix-dev/pixi/pull/1028)

#### Fixed

- Remove the need for a python interpreter in `pixi list` by @baszalmstra in [#1033](https://github.com/prefix-dev/pixi/pull/1033)


### [0.17.0] - 2024-03-19
#### ✨ Highlights

- This release greatly improves `pixi global` commands, thanks to @chawyehsu!
- We now support global (or local) configuration for pixi's own behavior, including mirrors, and OCI registries.
- We support channel mirrors for corporate environments!
- Faster `task` execution thanks to caching 🚀 Tasks that already executed successfully can be skipped based on the hash of the `inputs` and `outputs`.
- PyCharm and GitHub Actions integration thanks to @pavelzw – read more about it in the docs!

#### Added

- Add citation file by @ruben-arts in [#908](https://github.com/prefix-dev/pixi/pull/908)
- Add a pixi badge by @ruben-arts in [#961](https://github.com/prefix-dev/pixi/pull/961)
- Add deserialization of pypi source dependencies from toml by @ruben-arts and @wolf in [#895](https://github.com/prefix-dev/pixi/pull/895) [#984](https://github.com/prefix-dev/pixi/pull/984)
- Implement mirror and OCI settings by @wolfv in [#988](https://github.com/prefix-dev/pixi/pull/988)
- Implement `inputs` and `outputs` hash based task skipping by @wolfv in [#933](https://github.com/prefix-dev/pixi/pull/933)

#### Changed

- Refined global upgrade commands by @chawyehsu in [#948](https://github.com/prefix-dev/pixi/pull/948)
- Global upgrade supports matchspec by @chawyehsu in [#962](https://github.com/prefix-dev/pixi/pull/962)
- Improve `pixi search` with platform selection and making limit optional by @wolfv in [#979](https://github.com/prefix-dev/pixi/pull/979)
- Implement global config options by @wolfv in [#960](https://github.com/prefix-dev/pixi/pull/960) [#1015](https://github.com/prefix-dev/pixi/pull/1015) [#1019](https://github.com/prefix-dev/pixi/pull/1019)
- Update auth to use rattler cli by @kassoulait by @ruben-arts in [#986](https://github.com/prefix-dev/pixi/pull/986)

#### Documentation

- Remove cache: true from setup-pixi by @pavelzw in [#950](https://github.com/prefix-dev/pixi/pull/950)
- Add GitHub Actions documentation by @pavelzw in [#955](https://github.com/prefix-dev/pixi/pull/955)
- Add PyCharm documentation by @pavelzw in [#974](https://github.com/prefix-dev/pixi/pull/974)
- Mention `watch_file` in direnv usage by @pavelzw in [#983](https://github.com/prefix-dev/pixi/pull/983)
- Add tip to help users when no PROFILE file exists by @ruben-arts in [#991](https://github.com/prefix-dev/pixi/pull/991)
- Move yaml comments into mkdocs annotations by @pavelzw in [#1003](https://github.com/prefix-dev/pixi/pull/1003)
- Fix --env and extend actions examples by @ruben-arts in [#1005](https://github.com/prefix-dev/pixi/pull/1005)
- Add Wflow to projects built with pixi by @Hofer-Julian in [#1006](https://github.com/prefix-dev/pixi/pull/1006)
- Removed `linenums` to avoid buggy visualization by @ruben-arts in [#1002](https://github.com/prefix-dev/pixi/pull/1002)
- Fix typos by @pavelzw in [#1016](https://github.com/prefix-dev/pixi/pull/1016)

#### Fixed

- Pypi dependencies not being removed by @tdejager in [#952](https://github.com/prefix-dev/pixi/pull/952)
- Permissions for lint pr by @ruben-arts in [#852](https://github.com/prefix-dev/pixi/pull/852)
- Install Windows executable with `install.sh` in Git Bash by @jdblischak in [#966](https://github.com/prefix-dev/pixi/pull/966)
- Proper scanning of the conda-meta folder for `json` entries by @wolfv in [#971](https://github.com/prefix-dev/pixi/pull/971)
- Global shim scripts for Windows by @wolfv in [#975](https://github.com/prefix-dev/pixi/pull/975)
- Correct fish prompt by @wolfv in [#981](https://github.com/prefix-dev/pixi/pull/981)
- Prefix_file rename by @ruben-arts in [#959](https://github.com/prefix-dev/pixi/pull/959)
- Conda transitive dependencies of pypi packages are properly extracted by @baszalmstra in [#967](https://github.com/prefix-dev/pixi/pull/967)
- Make tests more deterministic and use single * for glob expansion by @wolfv in [#987](https://github.com/prefix-dev/pixi/pull/987)
- Create conda-meta/history file by @pavelzw in [#995](https://github.com/prefix-dev/pixi/pull/995)
- Pypi dependency parsing was too lenient by @wolfv in [#984](https://github.com/prefix-dev/pixi/pull/984)
- Add reactivation of the environment in pixi shell by @wolfv in [#982](https://github.com/prefix-dev/pixi/pull/982)
- Add `tool` to strict json schema by @ruben-arts in [#969](https://github.com/prefix-dev/pixi/pull/969)

#### New Contributors
* @jdblischak made their first contribution in [#966](https://github.com/prefix-dev/pixi/pull/966)
* @kassoulait made their first contribution in [#986](https://github.com/prefix-dev/pixi/pull/986)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.16.1..v0.17.0)

### [0.16.1] - 2024-03-11

#### Fixed
- Parse lockfile matchspecs lenient, fixing bug introduced in `0.16.0` by @ruben-arts in [#951](https://github.com/prefix-dev/pixi/pull/951)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.16.0..v0.16.1)

### [0.16.0] - 2024-03-09
#### ✨ Highlights
- This release removes [`rip`](https://github.com/prefix-dev/rip) and add [`uv`](https://github.com/astral-sh/uv) as the PyPI resolver and installer.

#### Added
- Add tcsh install support by @obust in [#898](https://github.com/prefix-dev/pixi/pull/898)
- Add user agent to pixi http client by @baszalmstra in [#892](https://github.com/prefix-dev/pixi/pull/892)
- Add a schema for the pixi.toml by @ruben-arts in [#936](https://github.com/prefix-dev/pixi/pull/936)

#### Changed
- Switch from rip to uv by @tdejager in [#863](https://github.com/prefix-dev/pixi/pull/863)
- Move uv options into context by @tdejager in [#911](https://github.com/prefix-dev/pixi/pull/911)
- Add Deltares projects to Community.md by @Hofer-Julian in [#920](https://github.com/prefix-dev/pixi/pull/920)
- Upgrade to uv 0.1.16, updated for changes in the API by @tdejager in [#935](https://github.com/prefix-dev/pixi/pull/935)

#### Fixed
- Made the uv re-install logic a bit more clear by @tdejager in [#894](https://github.com/prefix-dev/pixi/pull/894)
- Avoid duplicate pip dependency while importing environment.yaml by @sumanth-manchala in [#890](https://github.com/prefix-dev/pixi/pull/890)
- Handle custom channels when importing from env yaml by @sumanth-manchala in [#901](https://github.com/prefix-dev/pixi/pull/901)
- Pip editable installs getting uninstalled by @renan-r-santos in [#902](https://github.com/prefix-dev/pixi/pull/902)
- Highlight pypi deps in pixi list by @sumanth-manchala in [#907](https://github.com/prefix-dev/pixi/pull/907)
- Default to the default environment if possible by @ruben-arts in [#921](https://github.com/prefix-dev/pixi/pull/921)
- Switching channels by @baszalmstra in [#923](https://github.com/prefix-dev/pixi/pull/923)
- Use correct name of the channel on adding by @ruben-arts in [#928](https://github.com/prefix-dev/pixi/pull/928)
- Turn back on jlap for faster repodata fetching by @ruben-arts in [#937](https://github.com/prefix-dev/pixi/pull/937)
- Remove dists site-packages's when python interpreter changes by @tdejager in [#896](https://github.com/prefix-dev/pixi/pull/896)

#### New Contributors
* @obust made their first contribution in [#898](https://github.com/prefix-dev/pixi/pull/898)
* @renan-r-santos made their first contribution in [#902](https://github.com/prefix-dev/pixi/pull/902)

[Full Commit history](https://github.com/prefix-dev/pixi/compare/v0.15.2..v0.16.0)


### [0.15.2] - 2024-02-29

#### Changed
- Add more info to a failure of activation by @ruben-arts in [#873](https://github.com/prefix-dev/pixi/pull/873)

#### Fixed
- Improve global list UX when there is no global env dir created by @sumanth-manchala in [#865](https://github.com/prefix-dev/pixi/pull/865)
- Update rattler to `v0.19.0` by @AliPiccioniQC in [#885](https://github.com/prefix-dev/pixi/pull/885)
- Error on `pixi run` if platform is not supported by @ruben-arts in [#878](https://github.com/prefix-dev/pixi/pull/878)


#### New Contributors
- @sumanth-manchala made their first contribution in [#865](https://github.com/prefix-dev/pixi/pull/865)
- @AliPiccioniQC made their first contribution in [#885](https://github.com/prefix-dev/pixi/pull/885)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.15.1..v0.15.2)


### [0.15.1] - 2024-02-26

#### Added
- Add prefix to project info json output by @baszalmstra in [#859](https://github.com/prefix-dev/pixi/pull/859)

#### Changed
- New `pixi global list` display format by @chawyehsu in [#723](https://github.com/prefix-dev/pixi/pull/723)
- Add direnv usage by @pavelzw in [#845](https://github.com/prefix-dev/pixi/pull/845)
- Add docker example by @pavelzw in [#846](https://github.com/prefix-dev/pixi/pull/846)
- Install/remove multiple packages globally by @chawyehsu in [#854](https://github.com/prefix-dev/pixi/pull/854)

#### Fixed
- Prefix file in `init --import` by @ruben-arts in [#855](https://github.com/prefix-dev/pixi/pull/855)
- Environment and feature names in pixi info --json by @baszalmstra in [#857](https://github.com/prefix-dev/pixi/pull/857)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.15.0..v0.15.1)

### [0.15.0] - 2024-02-23

#### ✨ Highlights
- `[pypi-dependencies]` now get build in the created environment so it uses the conda installed build tools.
- `pixi init --import env.yml` to import an existing conda environment file.
- `[target.unix.dependencies]` to specify dependencies for unix systems instead of per platform.

> [!WARNING]
> This versions build failed, use `v0.15.1`

#### Added
- pass environment variables during pypi resolution and install ([#818](https://github.com/prefix-dev/pixi/pull/818))
- skip micromamba style selector lines and warn about them ([#830](https://github.com/prefix-dev/pixi/pull/830))
- add import yml flag ([#792](https://github.com/prefix-dev/pixi/pull/792))
- check duplicate dependencies ([#717](https://github.com/prefix-dev/pixi/pull/717))
- *(ci)* check conventional PR title ([#820](https://github.com/prefix-dev/pixi/pull/820))
- add `--feature` to `pixi add` ([#803](https://github.com/prefix-dev/pixi/pull/803))
- add windows, macos, linux and unix to targets ([#832](https://github.com/prefix-dev/pixi/pull/832))

#### Fixed
- cache and retry pypi name mapping ([#839](https://github.com/prefix-dev/pixi/pull/839))
- check duplicates while adding dependencies ([#829](https://github.com/prefix-dev/pixi/pull/829))
- logic `PIXI_NO_PATH_UPDATE` variable ([#822](https://github.com/prefix-dev/pixi/pull/822))

#### Other
- add `mike` to the documentation and update looks ([#809](https://github.com/prefix-dev/pixi/pull/809))
- add instructions for installing on Alpine Linux ([#828](https://github.com/prefix-dev/pixi/pull/828))
- more error reporting in `self-update` ([#823](https://github.com/prefix-dev/pixi/pull/823))
- disabled `jlap` for now ([#836](https://github.com/prefix-dev/pixi/pull/823))

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.14.0..v0.15.0)

### [0.14.0] - 2024-02-15

#### ✨ Highlights
Now, `solve-groups` can be used in `[environments]` to ensure dependency alignment across different environments without simultaneous installation.
This feature is particularly beneficial for managing identical dependencies in `test` and `production` environments.
Example configuration:

```toml
[environments]
test = { features = ["prod", "test"], solve-groups = ["group1"] }
prod = { features = ["prod"], solve-groups = ["group1"] }
```
This setup simplifies managing dependencies that must be consistent across `test` and `production`.

#### Added
- Add index field to pypi requirements by @vlad-ivanov-name in [#784](https://github.com/prefix-dev/pixi/pull/784)
- Add `-f`/`--feature` to the `pixi project platform` command by @ruben-arts in [#785](https://github.com/prefix-dev/pixi/pull/785)
- Warn user when unused features are defined by @ruben-arts in [#762](https://github.com/prefix-dev/pixi/pull/762)
- Disambiguate tasks interactive by @baszalmstra in [#766](https://github.com/prefix-dev/pixi/pull/766)
- Solve groups for conda by @baszalmstra in [#783](https://github.com/prefix-dev/pixi/pull/783)
- Pypi solve groups by @baszalmstra in [#802](https://github.com/prefix-dev/pixi/pull/802)
- Enable reflinks by @baszalmstra in [#729](https://github.com/prefix-dev/pixi/pull/729)

#### Changed
- Add environment name to the progress by @ruben-arts in [#788](https://github.com/prefix-dev/pixi/pull/788)
- Set color scheme by @ruben-arts in [#773](https://github.com/prefix-dev/pixi/pull/773)
- Update lock on `pixi list` by @ruben-arts in [#775](https://github.com/prefix-dev/pixi/pull/775)
- Use default env if task available in it. by @ruben-arts in [#772](https://github.com/prefix-dev/pixi/pull/772)
- Color environment name in install step by @ruben-arts in [#795](https://github.com/prefix-dev/pixi/pull/795)

#### Fixed
- Running cuda env and using those tasks. by @ruben-arts in [#764](https://github.com/prefix-dev/pixi/pull/764)
- Make svg a gif by @ruben-arts in [#782](https://github.com/prefix-dev/pixi/pull/782)
- Fmt by @ruben-arts
- Check for correct platform in task env creation by @ruben-arts in [#759](https://github.com/prefix-dev/pixi/pull/759)
- Remove using source name by @ruben-arts in [#765](https://github.com/prefix-dev/pixi/pull/765)
- Auto-guessing of the shell in the `shell-hook` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/811
- `sdist` with direct references by @nichmor in https://github.com/prefix-dev/pixi/pull/813

#### Miscellaneous
- Add slim-trees to community projects by @pavelzw in [#760](https://github.com/prefix-dev/pixi/pull/760)
- Add test to default env in polarify example
- Add multiple machine example by @ruben-arts in [#757](https://github.com/prefix-dev/pixi/pull/757)
- Add more documentation on `environments` by @ruben-arts in [#790](https://github.com/prefix-dev/pixi/pull/790)
- Update rip and rattler by @wolfv in [#798](https://github.com/prefix-dev/pixi/pull/798)
- Rattler 0.18.0 by @baszalmstra in [#805](https://github.com/prefix-dev/pixi/pull/805)
- Rip 0.8.0 by @nichmor in [#806](https://github.com/prefix-dev/pixi/pull/806)
- Fix authentication path by @pavelzw in [#796](https://github.com/prefix-dev/pixi/pull/796)
- Initial addition of integration test by @ruben-arts in https://github.com/prefix-dev/pixi/pull/804


#### New Contributors
* @vlad-ivanov-name made their first contribution in [#784](https://github.com/prefix-dev/pixi/pull/784)
* @nichmor made their first contribution in [#806](https://github.com/prefix-dev/pixi/pull/806)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.13.0..v0.14.0)

### [0.13.0] - 2024-02-01
#### ✨ Highlights
This release is pretty crazy in amount of features! The major ones are:
- We added support for multiple environments. :tada: Checkout the [documentation](https://pixi.sh/configuration/#the-feature-and-environments-tables)
- We added support for `sdist` installation, which greatly improves the amount of packages that can be installed from PyPI. :rocket:

> [!IMPORTANT]
>
> Renaming of `PIXI_PACKAGE_*` variables:
> ```
> PIXI_PACKAGE_ROOT -> PIXI_PROJECT_ROOT
> PIXI_PACKAGE_NAME ->  PIXI_PROJECT_NAME
> PIXI_PACKAGE_MANIFEST -> PIXI_PROJECT_MANIFEST
> PIXI_PACKAGE_VERSION -> PIXI_PROJECT_VERSION
> PIXI_PACKAGE_PLATFORMS -> PIXI_ENVIRONMENT_PLATFORMS
> ```
> Check documentation here: https://pixi.sh/environment/

> [!IMPORTANT]
>
> The `.pixi/env/` folder has been moved to accommodate multiple environments.
> If you only have one environment it is now named `.pixi/envs/default`.

#### Added
- Add support for multiple environment:
  - Update to rattler lock v4 by @baszalmstra in [#698](https://github.com/prefix-dev/pixi/pull/698)
  - Multi-env installation and usage by @baszalmstra in [#721](https://github.com/prefix-dev/pixi/pull/721)
  - Update all environments in the lock-file when requesting an environment by @baszalmstra in [#711](https://github.com/prefix-dev/pixi/pull/711)
  - Run tasks in the env they are defined by @baszalmstra in [#731](https://github.com/prefix-dev/pixi/pull/731)
  - `polarify` use-case as an example by @ruben-arts in [#735](https://github.com/prefix-dev/pixi/pull/735)
  - Make environment name parsing strict by @ruben-arts in [#673](https://github.com/prefix-dev/pixi/pull/673)
  - Use named environments (only "default" for now) by @ruben-arts in [#674](https://github.com/prefix-dev/pixi/pull/674)
  - Use task graph instead of traversal by @baszalmstra in [#725](https://github.com/prefix-dev/pixi/pull/725)
  - Multi env documentation by @ruben-arts in [#703](https://github.com/prefix-dev/pixi/pull/703)
  - `pixi info -e/--environment` option by @ruben-arts in [#676](https://github.com/prefix-dev/pixi/pull/676)
  - `pixi channel add -f/--feature` option by @ruben-arts in [#700](https://github.com/prefix-dev/pixi/pull/700)
  - `pixi channel remove -f/--feature` option by @ruben-arts in [#706](https://github.com/prefix-dev/pixi/pull/706)
  - `pixi remove -f/--feature` option by @ruben-arts in [#680](https://github.com/prefix-dev/pixi/pull/680)
  - `pixi task list -e/--environment` option by @ruben-arts in [#694](https://github.com/prefix-dev/pixi/pull/694)
  - `pixi task remove -f/--feature` option by @ruben-arts in [#694](https://github.com/prefix-dev/pixi/pull/694)
  - `pixi install -e/--environment` option by @ruben-arts in [#722](https://github.com/prefix-dev/pixi/pull/722)


- Support for sdists in `pypi-dependencies` by @tdejager in [#664](https://github.com/prefix-dev/pixi/pull/664)
- Add pre-release support to `pypi-dependencies` by @tdejager in [#716](https://github.com/prefix-dev/pixi/pull/716)


- Support adding dependencies for project's unsupported platforms by @orhun in [#668](https://github.com/prefix-dev/pixi/pull/668)
- Add `pixi list` command by @hadim in [#665](https://github.com/prefix-dev/pixi/pull/665)
- Add `pixi shell-hook` command by @orhun in [#672](https://github.com/prefix-dev/pixi/pull/672)[#679](https://github.com/prefix-dev/pixi/pull/679) [#684](https://github.com/prefix-dev/pixi/pull/684)
- Use env variable to configure locked, frozen and color by @hadim in [#726](https://github.com/prefix-dev/pixi/pull/726)
- `pixi self-update` by @hadim in [#675](https://github.com/prefix-dev/pixi/pull/675)
- Add `PIXI_NO_PATH_UPDATE` for PATH update suppression by @chawyehsu in [#692](https://github.com/prefix-dev/pixi/pull/692)
- Set the cache directory by @ruben-arts in [#683](https://github.com/prefix-dev/pixi/pull/683)


#### Changed
- Use consistent naming for tests module by @orhun in [#678](https://github.com/prefix-dev/pixi/pull/678)
- Install pixi and add to the path in docker example by @ruben-arts in [#743](https://github.com/prefix-dev/pixi/pull/743)
- Simplify the deserializer of `PyPiRequirement` by @orhun in [#744](https://github.com/prefix-dev/pixi/pull/744)
- Use `tabwriter` instead of `comfy_table` by @baszalmstra in [#745](https://github.com/prefix-dev/pixi/pull/745)
- Document environment variables by @ruben-arts in [#746](https://github.com/prefix-dev/pixi/pull/746)

#### Fixed
- Quote part of the task that has brackets (`[ or ]`) by @JafarAbdi in [#677](https://github.com/prefix-dev/pixi/pull/677)
- Package clobber and `__pycache__` removal issues by @wolfv in [#573](https://github.com/prefix-dev/pixi/pull/573)
- Non-global reqwest client by @tdejager in [#693](https://github.com/prefix-dev/pixi/pull/693)
- Fix broken pipe error during search by @orhun in [#699](https://github.com/prefix-dev/pixi/pull/699)
- Make `pixi search` result correct by @chawyehsu in [#713](https://github.com/prefix-dev/pixi/pull/713)
- Allow the tasks for all platforms to be shown in `pixi info` by @ruben-arts in [#728](https://github.com/prefix-dev/pixi/pull/728)
- Flaky tests while installing pypi dependencies by @baszalmstra in [#732](https://github.com/prefix-dev/pixi/pull/732)
- Linux install script by @mariusvniekerk in [#737](https://github.com/prefix-dev/pixi/pull/737)
- Download wheels in parallel to avoid deadlock by @baszalmstra in [#752](https://github.com/prefix-dev/pixi/pull/752)

#### New Contributors
* @JafarAbdi made their first contribution in [#677](https://github.com/prefix-dev/pixi/pull/677)
* @mariusvniekerk made their first contribution in [#737](https://github.com/prefix-dev/pixi/pull/737)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.12.0..v0.13.0)


### [0.12.0] - 2024-01-15
#### ✨ Highlights

- Some great community contributions, `pixi global upgrade`, `pixi project version` commands, a `PIXI_HOME` variable.
- A ton of refactor work to prepare for the [multi-environment](https://pixi.sh/design_proposals/multi_environment_proposal/) feature.
  - Note that there are no extra environments created yet, but you can just specify them in the `pixi.toml` file already.
  - Next we'll build the actual environments.

#### Added
- Add `global upgrade` command to pixi by @trueleo in [#614](https://github.com/prefix-dev/pixi/pull/614)
- Add configurable `PIXI_HOME` by @chawyehsu in [#627](https://github.com/prefix-dev/pixi/pull/627)
- Add `--pypi` option to `pixi remove` by @marcelotrevisani in https://github.com/prefix-dev/pixi/pull/602
- PrioritizedChannels to specify channel priority by @ruben-arts in https://github.com/prefix-dev/pixi/pull/658
- Add `project version {major,minor,patch}` CLIs by @hadim in https://github.com/prefix-dev/pixi/pull/633


#### Changed
- Refactored project model using targets, features and environments by @baszalmstra in https://github.com/prefix-dev/pixi/pull/616
- Move code from `Project` to `Environment` by @baszalmstra in [#630](https://github.com/prefix-dev/pixi/pull/630)
- Refactored `system-requirements` from Environment by @baszalmstra in [#632](https://github.com/prefix-dev/pixi/pull/632)
- Extract `activation.scripts` into Environment by @baszalmstra in [#659](https://github.com/prefix-dev/pixi/pull/659)
- Extract `pypi-dependencies` from Environment by @baszalmstra in https://github.com/prefix-dev/pixi/pull/656
- De-serialization of `features` and `environments` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/636

#### Fixed
- Make install.sh also work with wget if curl is not available by @wolfv in [#644](https://github.com/prefix-dev/pixi/pull/644)
- Use source build for rattler by @ruben-arts
- Check for pypi-dependencies before amending the pypi purls by @ruben-arts in [#661](https://github.com/prefix-dev/pixi/pull/661)
- Don't allow the use of reflinks by @ruben-arts in [#662](https://github.com/prefix-dev/pixi/pull/662)

#### Removed
- Remove windows and unix system requirements by @baszalmstra in [#635](https://github.com/prefix-dev/pixi/pull/635)

#### Documentation
- Document the channel logic by @ruben-arts in https://github.com/prefix-dev/pixi/pull/610
- Update the instructions for installing on Arch Linux by @orhun in https://github.com/prefix-dev/pixi/pull/653
- Update Community.md by @KarelZe in https://github.com/prefix-dev/pixi/pull/654
- Replace contributions.md with contributing.md and make it more standardized by @ruben-arts in https://github.com/prefix-dev/pixi/pull/649
- Remove `windows` and `unix` system requirements by @baszalmstra in https://github.com/prefix-dev/pixi/pull/635
- Add `CODE_OF_CONDUCT.md` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/648
- Removed remaining .ps1 references by @bahugo in https://github.com/prefix-dev/pixi/pull/643

#### New Contributors
* @marcelotrevisani made their first contribution in https://github.com/prefix-dev/pixi/pull/602
* @trueleo made their first contribution in https://github.com/prefix-dev/pixi/pull/614
* @bahugo made their first contribution in https://github.com/prefix-dev/pixi/pull/643
* @KarelZe made their first contribution in https://github.com/prefix-dev/pixi/pull/654

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.11.0...v0.12.0

### [0.11.1] - 2024-01-06

#### Fixed
- Upgrading rattler to fix `pixi auth` in [#642](https://github.com/prefix-dev/pixi/pull/642)

### [0.11.0] - 2024-01-05
#### ✨ Highlights

- Lots of important and preparations for the pypi `sdist` and multi environment feature
- Lots of new contributors that help `pixi` improve!

#### Added
- Add new commands for `pixi project {version|channel|platform|description}` by @hadim in [#579](https://github.com/prefix-dev/pixi/pull/579)
- Add dependabot.yml by @pavelzw in [#606](https://github.com/prefix-dev/pixi/pull/606)

#### Changed
- `winget-releaser` gets correct identifier by @ruben-arts in [#561](https://github.com/prefix-dev/pixi/pull/561)
- Task run code by @baszalmstra in [#556](https://github.com/prefix-dev/pixi/pull/556)
- No ps1 in activation scripts by @ruben-arts in [#563](https://github.com/prefix-dev/pixi/pull/563)
- Changed some names for clarity by @tdejager in [#568](https://github.com/prefix-dev/pixi/pull/568)
- Change font and make it dark mode by @ruben-arts in [#576](https://github.com/prefix-dev/pixi/pull/576)
- Moved pypi installation into its own module by @tdejager in [#589](https://github.com/prefix-dev/pixi/pull/589)
- Move alpha to beta feature and toggle it off with env var by @ruben-arts in [#604](https://github.com/prefix-dev/pixi/pull/604)
- Improve UX activation scripts by @ruben-arts in [#560](https://github.com/prefix-dev/pixi/pull/560)
- Add sanity check by @tdejager in [#569](https://github.com/prefix-dev/pixi/pull/569)
- Refactor manifest by @ruben-arts in [#572](https://github.com/prefix-dev/pixi/pull/572)
- Improve search by @Johnwillliam in [#578](https://github.com/prefix-dev/pixi/pull/578)
- Split pypi and conda solve steps by @tdejager in [#601](https://github.com/prefix-dev/pixi/pull/601)

#### Fixed
- Save file after lockfile is correctly updated by @ruben-arts in [#555](https://github.com/prefix-dev/pixi/pull/555)
- Limit the number of concurrent solves by @baszalmstra in [#571](https://github.com/prefix-dev/pixi/pull/571)
- Use project virtual packages in add command by @msegado in [#609](https://github.com/prefix-dev/pixi/pull/609)
- Improved mapped dependency by @ruben-arts in [#574](https://github.com/prefix-dev/pixi/pull/574)

#### Documentation
- Change font and make it dark mode by @ruben-arts in [#576](https://github.com/prefix-dev/pixi/pull/576)
- typo: no ps1 in activation scripts by @ruben-arts in [#563](https://github.com/prefix-dev/pixi/pull/563)
- Document adding CUDA to `system-requirements` by @ruben-arts in [#595](https://github.com/prefix-dev/pixi/pull/595)
- Multi env proposal documentation by @ruben-arts in [#584](https://github.com/prefix-dev/pixi/pull/584)
- Fix multiple typos in configuration.md by @SeaOtocinclus in [#608](https://github.com/prefix-dev/pixi/pull/608)
- Add multiple machines from one project example by @pavelzw in [#605](https://github.com/prefix-dev/pixi/pull/605)

#### New Contributors
* @hadim made their first contribution in [#579](https://github.com/prefix-dev/pixi/pull/579)
* @msegado made their first contribution in [#609](https://github.com/prefix-dev/pixi/pull/609)
* @Johnwillliam made their first contribution in [#578](https://github.com/prefix-dev/pixi/pull/578)
* @SeaOtocinclus made their first contribution in [#608](https://github.com/prefix-dev/pixi/pull/608)

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.10.0...v0.11.0

### [0.10.0] - 2023-12-8
#### Highlights
- Better `pypi-dependencies` support, now install even more of the pypi packages.
- `pixi add --pypi` command to add a pypi package to your project.

#### Added
* Use range (`>=1.2.3, <1.3`) when adding requirement, instead of `1.2.3.*` by @baszalmstra in https://github.com/prefix-dev/pixi/pull/536
* Update `rip` to fix  by @tdejager in https://github.com/prefix-dev/pixi/pull/543
  * Better Bytecode compilation (`.pyc`) support by @baszalmstra
  * Recognize `.data` directory `headers` by @baszalmstra
* Also print arguments given to a pixi task by @ruben-arts in https://github.com/prefix-dev/pixi/pull/545
* Add `pixi add --pypi` command by @ruben-arts in https://github.com/prefix-dev/pixi/pull/539

#### Fixed
* space in global install path by @ruben-arts in https://github.com/prefix-dev/pixi/pull/513
* Glibc version/family parsing by @baszalmstra in https://github.com/prefix-dev/pixi/pull/535
* Use `build` and `host` specs while getting the best version by @ruben-arts in https://github.com/prefix-dev/pixi/pull/538

#### Miscellaneous
* docs: add update manual by @ruben-arts in https://github.com/prefix-dev/pixi/pull/521
* add lightgbm demo by @partrita in https://github.com/prefix-dev/pixi/pull/492
* Update documentation link by @williamjamir in https://github.com/prefix-dev/pixi/pull/525
* Update Community.md by @jiaxiyang in https://github.com/prefix-dev/pixi/pull/527
* Add `winget` releaser by @ruben-arts in https://github.com/prefix-dev/pixi/pull/547
* Custom `rerun-sdk` example, force driven graph of `pixi.lock` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/548
* Better document pypi part by @ruben-arts in https://github.com/prefix-dev/pixi/pull/546

#### New Contributors
* @partrita made their first contribution in https://github.com/prefix-dev/pixi/pull/492
* @williamjamir made their first contribution in https://github.com/prefix-dev/pixi/pull/525
* @jiaxiyang made their first contribution in https://github.com/prefix-dev/pixi/pull/527

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.9.1...v0.10.0

### [0.9.1] - 2023-11-29
#### Highlights

* PyPI's `scripts` are now fixed. For example: https://github.com/prefix-dev/pixi/issues/516

#### Fixed
* Remove attr (unused) and update all dependencies by @wolfv in https://github.com/prefix-dev/pixi/pull/510
* Remove empty folders on python uninstall by @baszalmstra in https://github.com/prefix-dev/pixi/pull/512
* Bump `rip` to add scripts by @baszalmstra in https://github.com/prefix-dev/pixi/pull/517

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.9.0...v0.9.1

### [0.9.0] - 2023-11-28

#### Highlights
* You can now run `pixi remove`, `pixi rm` to remove a package from the environment
* Fix `pip install -e` issue that was created by release `v0.8.0` : https://github.com/prefix-dev/pixi/issues/507

#### Added
* `pixi remove` command by @Wackyator in https://github.com/prefix-dev/pixi/pull/483

#### Fixed
* Install entrypoints for `[pypi-dependencies]` @baszalmstra in https://github.com/prefix-dev/pixi/pull/508
* Only uninstall pixi installed packages by @baszalmstra in https://github.com/prefix-dev/pixi/pull/509

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.8.0...v0.9.0

### [0.8.0] - 2023-11-27

#### Highlights
* 🎉🐍`[pypi-dependencies]` ALPHA RELEASE🐍🎉, you can now add PyPI dependencies to your pixi project.
* UX of `pixi run` has been improved with better errors and showing what task is run.

> [!NOTE]
> `[pypi-dependencies]` support is still incomplete, missing functionality is listed here: https://github.com/orgs/prefix-dev/projects/6.
> Our intent is not to have 100% feature parity with `pip`, our goal is that you only need `pixi` for both conda and pypi packages alike.

#### Added
* Bump `rattler` @ruben-arts in https://github.com/prefix-dev/pixi/pull/496
* Implement lock-file satisfiability with `pypi-dependencies` by @baszalmstra in https://github.com/prefix-dev/pixi/pull/494
* List pixi tasks when `command not found` is returned by @ruben-arts in https://github.com/prefix-dev/pixi/pull/488
* Show which command is run as a pixi task by @ruben-arts in https://github.com/prefix-dev/pixi/pull/491 && https://github.com/prefix-dev/pixi/pull/493
* Add progress info to conda install by @baszalmstra in https://github.com/prefix-dev/pixi/pull/470
* Install pypi dependencies (alpha) by @baszalmstra in https://github.com/prefix-dev/pixi/pull/452

#### Fixed
* Add install scripts to `pixi.sh` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/458 && https://github.com/prefix-dev/pixi/pull/459 && https://github.com/prefix-dev/pixi/pull/460
* Fix `RECORD not found` issue by @baszalmstra in https://github.com/prefix-dev/pixi/pull/495
* Actually add to the `.gitignore` and give better errors by @ruben-arts in https://github.com/prefix-dev/pixi/pull/490
* Support macOS for `pypi-dependencies` by @baszalmstra in https://github.com/prefix-dev/pixi/pull/478
* Custom `pypi-dependencies` type by @ruben-arts in https://github.com/prefix-dev/pixi/pull/471
* `pypi-dependencies` parsing errors by @ruben-arts in https://github.com/prefix-dev/pixi/pull/479
* Progress issues by @baszalmstra in https://github.com/prefix-dev/pixi/pull/4

#### Miscellaneous
* Example: `ctypes` by @liquidcarbon in https://github.com/prefix-dev/pixi/pull/441
* Mention the AUR package by @orhun in https://github.com/prefix-dev/pixi/pull/464
* Update `rerun` example by @ruben-arts in https://github.com/prefix-dev/pixi/pull/489
* Document `pypi-dependencies` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/481
* Ignore docs paths on rust workflow by @ruben-arts in https://github.com/prefix-dev/pixi/pull/482
* Fix flaky tests, run serially by @baszalmstra in https://github.com/prefix-dev/pixi/pull/477


#### New Contributors
* @liquidcarbon made their first contribution in https://github.com/prefix-dev/pixi/pull/441
* @orhun made their first contribution in https://github.com/prefix-dev/pixi/pull/464

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.7.0...v0.8.0

### [0.7.0] - 2023-11-14

#### Highlights

- Channel priority: `channels = ["conda-forge", "pytorch"]` All packages found in conda-forge will not be taken from pytorch.
- Channel specific dependencies: `pytorch = { version="*", channel="pytorch"}`
- Autocompletion on `pixi run <TABTAB>`
- Moved all pixi documentation into this repo, try it with `pixi run docs`!
- Lots of new contributors!

#### Added
* Bump rattler to its newest version by @ruben-arts in https://github.com/prefix-dev/pixi/pull/395
    * Some notable changes:
        * Add channel priority (If a package is found in the first listed channel it will not be looked for in the other channels).
        * Fix JLAP using wrong hash.
        * Lockfile forward compatibility error.
* Add nushell support by @wolfv in https://github.com/prefix-dev/pixi/pull/360
* Autocomplete tasks on `pixi run` for `bash` and `zsh` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/390
* Add prefix location file to avoid copy error by @ruben-arts in https://github.com/prefix-dev/pixi/pull/422
* Channel specific dependencies `python = { version = "*" channel="conda-forge" }` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/439

#### Changed
* `project.version` as optional field in the `pixi.toml` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/400

#### Fixed
* Deny unknown fields in `pixi.toml` to help users find errors by @ruben-arts in https://github.com/prefix-dev/pixi/pull/396
* `install.sh` to create dot file if not present by @humphd in https://github.com/prefix-dev/pixi/pull/408
* Ensure order of repodata fetches by @baszalmstra in https://github.com/prefix-dev/pixi/pull/405
* Strip Linux binaries by @baszalmstra in https://github.com/prefix-dev/pixi/pull/414
* Sort `task list` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/431
* Fix `global install` path on windows by @ruben-arts in https://github.com/prefix-dev/pixi/pull/449
* Let `PIXI_BIN_PATH` use backslashes by @Hofer-Julian in https://github.com/prefix-dev/pixi/pull/442
* Print more informative error if created file is empty by @traversaro in https://github.com/prefix-dev/pixi/pull/447

#### Docs
* Move to `mkdocs` with all documentation by @ruben-arts in https://github.com/prefix-dev/pixi/pull/435
* Fix typing errors by @FarukhS52 in https://github.com/prefix-dev/pixi/pull/426
* Add social cards to the pages by @ruben-arts in https://github.com/prefix-dev/pixi/pull/445
* Enhance README.md: Added Table of Contents, Grammar Improvements by @adarsh-jha-dev in https://github.com/prefix-dev/pixi/pull/421
* Adding conda-auth to community examples by @travishathaway in https://github.com/prefix-dev/pixi/pull/433
* Minor grammar correction by @tylere in https://github.com/prefix-dev/pixi/pull/406
* Make capitalization of tab titles consistent by @tylere in https://github.com/prefix-dev/pixi/pull/407

#### New Contributors
* @tylere made their first contribution in https://github.com/prefix-dev/pixi/pull/406
* @humphd made their first contribution in https://github.com/prefix-dev/pixi/pull/408
* @adarsh-jha-dev made their first contribution in https://github.com/prefix-dev/pixi/pull/421
* @FarukhS52 made their first contribution in https://github.com/prefix-dev/pixi/pull/426
* @travishathaway made their first contribution in https://github.com/prefix-dev/pixi/pull/433
* @traversaro made their first contribution in https://github.com/prefix-dev/pixi/pull/447

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.6.0...v0.7.0

### [0.6.0] - 2023-10-17

#### Highlights
This release fixes some bugs and adds the `--cwd` option to the tasks.

#### Fixed
* Improve shell prompts by @ruben-arts in https://github.com/prefix-dev/pixi/pull/385 https://github.com/prefix-dev/pixi/pull/388
* Change `--frozen` logic to error when there is no lockfile by @ruben-arts in https://github.com/prefix-dev/pixi/pull/373
* Don't remove the '.11' from 'python3.11' binary file name by @ruben-arts in https://github.com/prefix-dev/pixi/pull/366

#### Changed
* Update `rerun` example to v0.9.1 by @ruben-arts in https://github.com/prefix-dev/pixi/pull/389

#### Added
* Add the current working directory (`--cwd`) in `pixi tasks` by @ruben-arts in https://github.com/prefix-dev/pixi/pull/380

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.5.0...v0.6.0

### [0.5.0] - 2023-10-03

#### Highlights

We rebuilt `pixi shell`, fixing the fact that your `rc` file would overrule the environment activation.

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

### [0.4.0] - 2023-09-22

#### Highlights

This release adds the start of a new cli command `pixi project` which will allow users to interact with the project configuration from the command line.

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

#### New Contributors
* @yarikoptic made their first contribution in https://github.com/prefix-dev/pixi/pull/329
* @HaoZeke made their first contribution in https://github.com/prefix-dev/pixi/pull/339

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.3.0...v0.4.0

### [0.3.0] - 2023-09-11

#### Highlights

This releases fixes a lot of issues encountered by the community as well as some awesome community contributions like the addition of `pixi global list` and `pixi global remove`.

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

### [0.2.0] - 2023-08-22

#### Highlights
- Added `pixi search` command to search for packages, by @Wackyator. ([#244](https://github.com/prefix-dev/pixi/pull/244))
- Added target specific tasks, eg. `[target.win-64.tasks]`, by @ruben-arts. ([#269](https://github.com/prefix-dev/pixi/pull/269))
- Flaky install caused by the download of packages, by @baszalmstra. ([#281](https://github.com/prefix-dev/pixi/pull/281))

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

### [0.1.0] - 2023-08-11

As this is our first [Semantic Versioning](https://semver.org) release, we'll change from the prototype to the developing phase, as semver describes.
A 0.x release could be anything from a new major feature to a breaking change where the 0.0.x releases will be bugfixes or small improvements.

#### Highlights
- Update to the latest [rattler](https://github.com/mamba-org/rattler/releases/tag/v0.7.0) version, by @baszalmstra. ([#249](https://github.com/prefix-dev/pixi/pull/249))

#### Fixed
- Only add shebang to activation scripts on `unix` platforms, by @baszalmstra. ([#250](https://github.com/prefix-dev/pixi/pull/250))
- Use official crates.io releases for all dependencies, by @baszalmstra. ([#252](https://github.com/prefix-dev/pixi/pull/252))

### [0.0.8] - 2023-08-01

#### Highlights
- Much better error printing using `miette`, by @baszalmstra. ([#211](https://github.com/prefix-dev/pixi/pull/211))
- You can now use pixi on `aarch64-linux`, by @pavelzw.  ([#233](https://github.com/prefix-dev/pixi/pull/233))
- Use the Rust port of `libsolv` as the default solver, by @ruben-arts. ([#209](https://github.com/prefix-dev/pixi/pull/209))

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

### [0.0.7] - 2023-07-11

#### Highlights
- Transitioned the `run` subcommand to use the `deno_task_shell` for improved cross-platform functionality. More details in the [Deno Task Runner documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner).
- Added an `info` subcommand to retrieve system-specific information understood by `pixi`.

#### BREAKING CHANGES
- `[commands]` in the `pixi.toml` is now called `[tasks]`. ([#177](https://github.com/prefix-dev/pixi/pull/177))

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


### [0.0.6] - 2023-06-30

#### Highlights
Improving the reliability is important to us, so we added an integration testing framework, we can now test as close as possible to the CLI level using `cargo`.

#### Added
- An integration test harness, to test as close as possible to the user experience but in rust. ([#138](https://github.com/prefix-dev/pixi/pull/138), [#140](https://github.com/prefix-dev/pixi/pull/140), [#156](https://github.com/prefix-dev/pixi/pull/156))
- Add different levels of dependencies in preparation for `pixi build`, allowing `host-` and `build-` `dependencies` ([#149](https://github.com/prefix-dev/pixi/pull/149))

#### Fixed
- Use correct folder name on pixi init ([#144](https://github.com/prefix-dev/pixi/pull/144))
- Fix windows cli installer ([#152](https://github.com/prefix-dev/pixi/pull/152))
- Fix global install path variable ([#147](https://github.com/prefix-dev/pixi/pull/147))
- Fix macOS binary notarization ([#153](https://github.com/prefix-dev/pixi/pull/153))

### [0.0.5] - 2023-06-26

Fixing Windows installer build in CI. ([#145](https://github.com/prefix-dev/pixi/pull/145))

### [0.0.4] - 2023-06-26

#### Highlights

A new command, `auth` which can be used to authenticate the host of the package channels.
A new command, `shell` which can be used to start a shell in the pixi environment of a project.
A refactor of the `install` command which is changed to `global install` and the `install` command now installs a pixi project if you run it in the directory.
Platform specific dependencies using `[target.linux-64.dependencies]` instead of `[dependencies]` in the `pixi.toml`

Lots and lots of fixes and improvements to make it easier for this user, where bumping to the new version of [`rattler`](https://github.com/mamba-org/rattler/releases/tag/v0.4.0)
helped a lot.

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
