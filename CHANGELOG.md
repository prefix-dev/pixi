# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.24.1] - 2024-06-12
### 📃 Details
#### Fixed
- Replace http code %2b with + by @ruben-arts in [#1500](https://github.com/prefix-dev/pixi/pull/1500)

## [0.24.0] - 2024-06-12
### ✨ Highlights

- You can now run in a more isolated environment on `unix` machines, using `pixi run --clean-env TASK_NAME`.
- You can new easily clean your environment with `pixi clean` or the cache with `pixi clean cache`

### 📃 Details
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

## New Contributors
* @dhirschfeld made their first contribution in [#1458](https://github.com/prefix-dev/pixi/pull/1458)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.23.0..HEAD)

## [0.23.0] - 2024-05-27
### ✨ Highlights
- This release adds two new commands `pixi config` and `pixi update`
  - `pixi config` allows you to `edit`, `set`, `unset`, `append`, `prepend` and `list` your local/global or system configuration.
  - `pixi update` re-solves the full lockfile or use `pixi update PACKAGE` to only update `PACKAGE`, making sure your project is using the latest versions that the manifest allows for.

### 📃 Details
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

## New Contributors
* @tfriedel made their first contribution in [#1424](https://github.com/prefix-dev/pixi/pull/1424)
* @jjjermiah made their first contribution in [#1403](https://github.com/prefix-dev/pixi/pull/1403)
* @tobiasraabe made their first contribution in [#1396](https://github.com/prefix-dev/pixi/pull/1396)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.22.0..v0.23.0)

## [0.22.0] - 2024-05-13
### ✨ Highlights

- Support for source pypi dependencies through the cli:
  - `pixi add --pypi 'package @ package.whl'`, perfect for adding just build wheels to your environment in CI.
  - `pixi add --pypi 'package_from_git @ git+https://github.com/org/package.git'`, to add a package from a git repository.
  - `pixi add --pypi 'package_from_path @ file:///path/to/package' --editable`, to add a package from a local path.


### 📃 Details
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


## New Contributors
* @twrightsman made their first contribution in [#1368](https://github.com/prefix-dev/pixi/pull/1368)
* @notPlancha made their first contribution in [#1358](https://github.com/prefix-dev/pixi/pull/1358)
* @vigneshmanick made their first contribution in [#1336](https://github.com/prefix-dev/pixi/pull/1336)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.21.1..v0.22.0)


## [0.21.1] - 2024-05-07
### 📃 Details
#### Fixed
- Use read timeout, not global timeout by @wolfv in [#1329](https://github.com/prefix-dev/pixi/pull/1329)
- Channel priority logic by @ruben-arts in [#1332](https://github.com/prefix-dev/pixi/pull/1332)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.21.0..v0.21.1)

## [0.21.0] - 2024-05-06
### ✨ Highlights
- This release adds support for configuring PyPI settings globally, to use alternative PyPI indexes and load credentials with keyring.
- We now support cross-platform running, for `osx-64` on `osx-arm64` and `wasm` environments.
- There is now a `no-default-feature` option to simplify usage of environments.

### 📃 Details

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

## New Contributors
* @borchero made their first contribution in [#1317](https://github.com/prefix-dev/pixi/pull/1317)
* @JesperDramsch made their first contribution in [#1313](https://github.com/prefix-dev/pixi/pull/1313)
* @Hoxbro made their first contribution in [#1286](https://github.com/prefix-dev/pixi/pull/1286)
* @carschandler made their first contribution in [#1297](https://github.com/prefix-dev/pixi/pull/1297)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.20.1..v0.21.0)

## [0.20.1] - 2024-04-26
### ✨ Highlights
- Big improvements on the pypi-editable installs.


### 📃 Details
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

## New Contributors
* @bollwyvl made their first contribution in [#1265](https://github.com/prefix-dev/pixi/pull/1265)
* @bruchim-cisco made their first contribution in [#1267](https://github.com/prefix-dev/pixi/pull/1267)
* @zen-xu made their first contribution in [#1250](https://github.com/prefix-dev/pixi/pull/1250)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.20.0..v0.20.1)


## [0.20.0] - 2024-04-19
### ✨ Highlights

- We now support `env` variables in the `task` definition, these can also be used as default values for parameters in your task which you can overwrite with your shell's env variables.
e.g. `task = { cmd = "task to run", env = { VAR="value1", PATH="my/path:$PATH" } }`
- We made a big effort on fixing issues and improving documentation!

### 📃 Details
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

## New Contributors
* @glemaitre made their first contribution in [#1220](https://github.com/prefix-dev/pixi/pull/1220)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.19.1..v0.20.0)


## [0.19.1] - 2024-04-11
### ✨ Highlights
This fixes the issue where pixi would generate broken environments/lockfiles when a mapping for a brand-new version of a package is missing.

### 📃 Details
- Add fallback mechanism for missing mapping by @nichmor in [#1166](https://github.com/prefix-dev/pixi/pull/1166)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.19.0..v0.19.1)

## [0.19.0] - 2024-04-10
### ✨ Highlights
- This release adds a new `pixi tree` command to show the dependency tree of the project.
- Pixi now persists the manifest and environment when activating a shell, so you can use pixi as if you are in that folder while in the shell.

### 📃 Details
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

## New Contributors
* @abkfenris made their first contribution in [#1069](https://github.com/prefix-dev/pixi/pull/1069)
* @beenje made their first contribution in [#1129](https://github.com/prefix-dev/pixi/pull/1129)
* @jaimergp made their first contribution in [#1117](https://github.com/prefix-dev/pixi/pull/1117)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.18.0..v0.19.0)


## [0.18.0] - 2024-04-02
### ✨ Highlights
- This release adds support for `pyproject.toml`, now pixi reads from the `[tool.pixi]` table.
- We now support editable PyPI dependencies, and PyPI source dependencies, including `git`, `path`, and `url` dependencies.

> [!TIP]
> These new features are part of the ongoing effort to make pixi more flexible, powerful, and comfortable for the python users.
> They are still in progress so expect more improvements on these features soon, so please report any issues you encounter and follow our next releases!

### 📃 Details
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

## New Contributors
* @pya made their first contribution in [#1091](https://github.com/prefix-dev/pixi/pull/1091)
* @ytausch made their first contribution in [#1076](https://github.com/prefix-dev/pixi/pull/1076)
* @hack3ric made their first contribution in [#1045](https://github.com/prefix-dev/pixi/pull/1045)
* @olivier-lacroix made their first contribution in [#999](https://github.com/prefix-dev/pixi/pull/999)
* @henryiii made their first contribution in [#1063](https://github.com/prefix-dev/pixi/pull/1063)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.17.1..v0.18.0)

## [0.17.1] - 2024-03-21
### ✨ Highlights

A quick bug-fix release for `pixi list`.

### 📃 Details

#### Documentation

- Fix typo by @pavelzw in [#1028](https://github.com/prefix-dev/pixi/pull/1028)

#### Fixed

- Remove the need for a python interpreter in `pixi list` by @baszalmstra in [#1033](https://github.com/prefix-dev/pixi/pull/1033)


## [0.17.0] - 2024-03-19
### ✨ Highlights

- This release greatly improves `pixi global` commands, thanks to @chawyehsu!
- We now support global (or local) configuration for pixi's own behavior, including mirrors, and OCI registries.
- We support channel mirrors for corporate environments!
- Faster `task` execution thanks to caching 🚀 Tasks that already executed successfully can be skipped based on the hash of the `inputs` and `outputs`.
- PyCharm and GitHub Actions integration thanks to @pavelzw – read more about it in the docs!

### 📃 Details

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

## New Contributors
* @jdblischak made their first contribution in [#966](https://github.com/prefix-dev/pixi/pull/966)
* @kassoulait made their first contribution in [#986](https://github.com/prefix-dev/pixi/pull/986)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.16.1..v0.17.0)

## [0.16.1] - 2024-03-11
### 📃 Details
#### Fixed
- Parse lockfile matchspecs lenient, fixing bug introduced in `0.16.0` by @ruben-arts in [#951](https://github.com/prefix-dev/pixi/pull/951)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.16.0..v0.16.1)

## [0.16.0] - 2024-03-09
### ✨ Highlights
- This release removes [`rip`](https://github.com/prefix-dev/rip) and add [`uv`](https://github.com/astral-sh/uv) as the PyPI resolver and installer.

### 📃 Details
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

## New Contributors
* @obust made their first contribution in [#898](https://github.com/prefix-dev/pixi/pull/898)
* @renan-r-santos made their first contribution in [#902](https://github.com/prefix-dev/pixi/pull/902)

[Full Commit history](https://github.com/prefix-dev/pixi/compare/v0.15.2..v0.16.0)


## [0.15.2](https://github.com/prefix-dev/pixi/compare/v0.15.1...v0.15.2)  - 2024-02-29
### 📃 Details

#### Changed
- Add more info to a failure of activation by @ruben-arts in [#873](https://github.com/prefix-dev/pixi/pull/873)

#### Fixed
- Improve global list UX when there is no global env dir created by @sumanth-manchala in [#865](https://github.com/prefix-dev/pixi/pull/865)
- Update rattler to `v0.19.0` by @AliPiccioniQC in [#885](https://github.com/prefix-dev/pixi/pull/885)
- Error on `pixi run` if platform is not supported by @ruben-arts in [#878](https://github.com/prefix-dev/pixi/pull/878)


### New Contributors
- @sumanth-manchala made their first contribution in [#865](https://github.com/prefix-dev/pixi/pull/865)
- @AliPiccioniQC made their first contribution in [#885](https://github.com/prefix-dev/pixi/pull/885)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.15.1..v0.15.2)


## [0.15.1](https://github.com/prefix-dev/pixi/compare/v0.15.0...v0.15.1) - 2024-02-26
### 📃 Details

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

## [0.15.0](https://github.com/prefix-dev/pixi/compare/v0.14.0...v0.15.0) - 2024-02-23

## ✨ Highlights
- `[pypi-dependencies]` now get build in the created environment so it uses the conda installed build tools.
- `pixi init --import env.yml` to import an existing conda environment file.
- `[target.unix.dependencies]` to specify dependencies for unix systems instead of per platform.

> [!WARNING]
> This versions build failed, use `v0.15.1`

### 📃 Details
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

## [0.14.0] - 2024-02-15

### ✨ Highlights
Now, `solve-groups` can be used in `[environments]` to ensure dependency alignment across different environments without simultaneous installation.
This feature is particularly beneficial for managing identical dependencies in `test` and `production` environments.
Example configuration:

```toml
[environments]
test = { features = ["prod", "test"], solve-groups = ["group1"] }
prod = { features = ["prod"], solve-groups = ["group1"] }
```
This setup simplifies managing dependencies that must be consistent across `test` and `production`.

### 📃 Details

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


## New Contributors
* @vlad-ivanov-name made their first contribution in [#784](https://github.com/prefix-dev/pixi/pull/784)
* @nichmor made their first contribution in [#806](https://github.com/prefix-dev/pixi/pull/806)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.13.0..v0.14.0)

## [0.13.0] - 2024-02-01
### ✨ Highlights
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

### 📃 Details

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

## New Contributors
* @JafarAbdi made their first contribution in [#677](https://github.com/prefix-dev/pixi/pull/677)
* @mariusvniekerk made their first contribution in [#737](https://github.com/prefix-dev/pixi/pull/737)

[Full commit history](https://github.com/prefix-dev/pixi/compare/v0.12.0..v0.13.0)


## [0.12.0] - 2024-01-15
### ✨ Highlights

- Some great community contributions, `pixi global upgrade`, `pixi project version` commands, a `PIXI_HOME` variable.
- A ton of refactor work to prepare for the [multi-environment](https://pixi.sh/design_proposals/multi_environment_proposal/) feature.
  - Note that there are no extra environments created yet, but you can just specify them in the `pixi.toml` file already.
  - Next we'll build the actual environments.

### 📃 Details

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

## New Contributors
* @marcelotrevisani made their first contribution in https://github.com/prefix-dev/pixi/pull/602
* @trueleo made their first contribution in https://github.com/prefix-dev/pixi/pull/614
* @bahugo made their first contribution in https://github.com/prefix-dev/pixi/pull/643
* @KarelZe made their first contribution in https://github.com/prefix-dev/pixi/pull/654

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.11.0...v0.12.0

## [0.11.1] - 2024-01-06

### 📃 Details
#### Fixed
- Upgrading rattler to fix `pixi auth` in [#642](https://github.com/prefix-dev/pixi/pull/642)

## [0.11.0] - 2024-01-05
### ✨ Highlights

- Lots of important and preparations for the pypi `sdist` and multi environment feature
- Lots of new contributors that help `pixi` improve!

### 📃 Details
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

## New Contributors
* @hadim made their first contribution in [#579](https://github.com/prefix-dev/pixi/pull/579)
* @msegado made their first contribution in [#609](https://github.com/prefix-dev/pixi/pull/609)
* @Johnwillliam made their first contribution in [#578](https://github.com/prefix-dev/pixi/pull/578)
* @SeaOtocinclus made their first contribution in [#608](https://github.com/prefix-dev/pixi/pull/608)

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.10.0...v0.11.0

## [0.10.0] - 2023-12-8
### Highlights
- Better `pypi-dependencies` support, now install even more of the pypi packages.
- `pixi add --pypi` command to add a pypi package to your project.

### Details
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

## New Contributors
* @partrita made their first contribution in https://github.com/prefix-dev/pixi/pull/492
* @williamjamir made their first contribution in https://github.com/prefix-dev/pixi/pull/525
* @jiaxiyang made their first contribution in https://github.com/prefix-dev/pixi/pull/527

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.9.1...v0.10.0

## [0.9.1] - 2023-11-29
### Highlights

* PyPI's `scripts` are now fixed. For example: https://github.com/prefix-dev/pixi/issues/516

### Details
#### Fixed
* Remove attr (unused) and update all dependencies by @wolfv in https://github.com/prefix-dev/pixi/pull/510
* Remove empty folders on python uninstall by @baszalmstra in https://github.com/prefix-dev/pixi/pull/512
* Bump `rip` to add scripts by @baszalmstra in https://github.com/prefix-dev/pixi/pull/517

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.9.0...v0.9.1

## [0.9.0] - 2023-11-28

### Highlights
* You can now run `pixi remove`, `pixi rm` to remove a package from the environment
* Fix `pip install -e` issue that was created by release `v0.8.0` : https://github.com/prefix-dev/pixi/issues/507

### Details
#### Added
* `pixi remove` command by @Wackyator in https://github.com/prefix-dev/pixi/pull/483

#### Fixed
* Install entrypoints for `[pypi-dependencies]` @baszalmstra in https://github.com/prefix-dev/pixi/pull/508
* Only uninstall pixi installed packages by @baszalmstra in https://github.com/prefix-dev/pixi/pull/509

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.8.0...v0.9.0

## [0.8.0] - 2023-11-27

### Highlights
* 🎉🐍`[pypi-dependencies]` ALPHA RELEASE🐍🎉, you can now add PyPI dependencies to your pixi project.
* UX of `pixi run` has been improved with better errors and showing what task is run.

> [!NOTE]
> `[pypi-dependencies]` support is still incomplete, missing functionality is listed here: https://github.com/orgs/prefix-dev/projects/6.
> Our intent is not to have 100% feature parity with `pip`, our goal is that you only need `pixi` for both conda and pypi packages alike.

### Details
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


## New Contributors
* @liquidcarbon made their first contribution in https://github.com/prefix-dev/pixi/pull/441
* @orhun made their first contribution in https://github.com/prefix-dev/pixi/pull/464

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.7.0...v0.8.0

## [0.7.0] - 2023-11-14

### Highlights

- Channel priority: `channels = ["conda-forge", "pytorch"]` All packages found in conda-forge will not be taken from pytorch.
- Channel specific dependencies: `pytorch = { version="*", channel="pytorch"}`
- Autocompletion on `pixi run <TABTAB>`
- Moved all pixi documentation into this repo, try it with `pixi run docs`!
- Lots of new contributors!

### Details
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

## New Contributors
* @tylere made their first contribution in https://github.com/prefix-dev/pixi/pull/406
* @humphd made their first contribution in https://github.com/prefix-dev/pixi/pull/408
* @adarsh-jha-dev made their first contribution in https://github.com/prefix-dev/pixi/pull/421
* @FarukhS52 made their first contribution in https://github.com/prefix-dev/pixi/pull/426
* @travishathaway made their first contribution in https://github.com/prefix-dev/pixi/pull/433
* @traversaro made their first contribution in https://github.com/prefix-dev/pixi/pull/447

**Full Changelog**: https://github.com/prefix-dev/pixi/compare/v0.6.0...v0.7.0

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
