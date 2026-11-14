A Pixi workspace can support several operating systems and, within one operating system, several hardware variants such as CPU-only and CUDA machines.
This page starts with a workspace for several operating systems, adds CPU and CUDA variants, and then covers the full selection rules.

## Supporting several operating systems

List the conda subdirs your workspace supports in `workspace.platforms`:

```toml title="pixi.toml"
[workspace]
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]

[dependencies]
python = "3.15.*"

[tasks]
version = "python --version"
```

The environment is resolved separately for each platform, and `pixi.lock` stores one package set per platform.
Pixi resolves all of them on any machine, so a Linux machine also locks the macOS and Windows package sets.
`pixi install` and `pixi run` install the package set that fits the current machine.
[Target specifiers](#target-specifier) add dependencies, tasks, or activation scripts for individual platforms.

## Declared, detected, and required virtual packages

Some packages need features of the machine instead of other packages, for example a CUDA driver.
These are described via [virtual packages](../conda_ecosystem.md#virtual-packages-describing-the-host-system) such as `__cuda`.
Pixi uses them in three separate roles:

| Role | Example | Purpose |
|---|---|---|
| Declared | `cuda = "12.0"` in `workspace.platforms` | Adds exactly `__cuda=12.0` to the solve of that platform |
| Detected | `__cuda=12.9=0` reported by the machine | Describes what the current machine provides |
| Required | `__cuda >=12` in a locked package | States what a resolved package needs |

Pixi solves each platform with the virtual packages it declares, not with what the current machine reports.
For `cuda = "12.0"`, the solver sees exactly `__cuda=12.0`, so a CPU-only CI runner can lock it.
Which packages fit is up to their run dependencies and constraints, for example `__cuda >=12` in `cuda-version`'s `constrains`.
Whether the current machine can run the result is checked when installing.

`pixi.lock` stores the declarations of each platform and the requirements of each package, never the detected values.
With a `linux-64-cuda` platform declaring `cuda = "12.0"` and a plain `linux-64-cpu` platform, the relevant parts look like this:

```yaml title="pixi.lock"
platforms:
- name: linux-64-cuda
  subdir: linux-64
  virtual-packages:
  - __cuda=12.0
  - __glibc=2.28
  # ...
- name: linux-64-cpu
  subdir: linux-64
  virtual-packages:
  - __glibc=2.28
  # ...
environments:
  default:
    packages:
      linux-64-cuda:
      - conda: https://prefix.dev/conda-forge/noarch/cuda-version-12.9-h4f385c5_3.conda
      # ...
      linux-64-cpu:
      # ...
packages:
- conda: https://prefix.dev/conda-forge/noarch/cuda-version-12.9-h4f385c5_3.conda
  constrains:
  - __cuda >=12
  # ...
```

When installing `default`, Pixi tries `linux-64-cuda` first on a machine reporting CUDA 12.0 or newer.
It selects that platform only if its locked packages fit the machine; otherwise it tries `linux-64-cpu`.
[Which platform gets installed?](#which-platform-gets-installed) has the exact rules.

`cuda = "12.0"` declares the CUDA capability of the NVIDIA driver.
It does not install a driver, and it is not the CUDA toolkit.
Toolkit and runtime packages such as `cuda-version` or `cuda-runtime` are ordinary dependencies.
Virtual packages are never downloaded, and Pixi does not export their values as environment variables.

Host-oriented commands such as `pixi exec` and native `pixi global install` don't have declared workspace platforms, so they solve against the detected values.

## CPU and CUDA variants of one subdir

To support both CPU-only and CUDA machines on `linux-64`, declare two named platforms with the same subdir and give each its own dependencies:

```toml title="pixi.toml"
[workspace]
channels = ["conda-forge"]
platforms = [
  { name = "linux-64-cuda", platform = "linux-64", cuda = "12.0" },
  { name = "linux-64-cpu", platform = "linux-64" },
  "osx-arm64",
]

[dependencies]
python = "3.14.*"

[target.linux-64-cuda.dependencies]
pytorch-gpu = "*"

[target.linux-64-cpu.dependencies]
pytorch-cpu = "*"

[target.osx-arm64.dependencies]
pytorch = "*"
```

`pixi lock` resolves three package sets for the `default` environment, including the CUDA one on a machine without a GPU.

The CPU platform selects `pytorch-cpu` explicitly.
Leaving out the CUDA declaration only means that the solver can't use packages that require `__cuda`; it does not request a CPU build.

Target selectors match in two ways:

- A platform name such as `[target.linux-64-cuda]` matches exactly that platform.
- A subdir such as `[target.linux-64]` matches every platform with that subdir.
  Here it would apply to both `linux-64-cuda` and `linux-64-cpu`, and adding `pytorch-cpu` to the CUDA solve would conflict with `pytorch-gpu`.

`linux-64-cuda` is solved as if the machine had a driver for CUDA 12.0, and the run dependencies and constraints of the CUDA packages decide which builds fit that.

On a `linux-64` machine with CUDA 12.0 or newer, Pixi tries `linux-64-cuda` first.
If its locked packages don't fit the machine, Pixi tries `linux-64-cpu` instead.
Machines without a matching CUDA declaration try the CPU platform first.
Order matters: if `linux-64-cpu` came first and its locked packages fit, the CUDA platform would never be selected automatically.
`pixi run` keeps using an installed environment with its platform, so installing a driver later doesn't switch an existing CPU environment to CUDA.
Run `pixi install` to select again.
[Which platform gets installed?](#which-platform-gets-installed) has the complete rules.

The [PyTorch guide](../python/pytorch.md) covers pinning the CUDA version with `cuda-version` and installing PyTorch from PyPI.

## Target specifier

With the target specifier, you can overwrite the original configuration specifically for a single platform.
The selector is a platform name, a subdir, a [wildcard pattern](#wildcard-platform-selectors), or one of `unix`, `linux`, `win`, and `osx`.
A selector that doesn't match any platform in `workspace.platforms` produces a warning.

### Dependencies

It might happen that you want to install a certain dependency only on a specific platform, or you might want to use a different version on different platforms.

```toml title="pixi.toml"
[dependencies]
python = ">=3.12"

[target.win-64.dependencies]
msmpi = "*"
python = "3.12.*"
```

In the above example, we depend on `msmpi` only on Windows and pin `python` to `3.12` there.
This overrides the generic dependencies only for `win-64`.

You can use Pixi's CLI to add these dependencies to the manifest file.

```bash
pixi add --platform win-64 posix
```

This also works for the `host` and `build` dependencies.

```bash
pixi add --host --platform win-64 posix
pixi add --build --platform osx-64 clang
```

Which results in this.

```toml title="pixi.toml"
[target.win-64.host-dependencies]
posix = "1.0.0.*"

[target.osx-64.build-dependencies]
clang = "16.0.6.*"
```

### Activation

Activation scripts are often specific to a shell, for example `bash` on Unix and `bat` on Windows.
Define them per target:

```toml title="pixi.toml"
[activation]
scripts = ["setup.sh", "local_setup.bash"]

[target.win-64.activation]
scripts = ["setup.bat", "local_setup.bat"]
```

On `win-64`, Pixi runs only the target scripts, not the scripts in the default `activation.scripts`.

### Wildcard platform selectors

When several [workspace platforms](#declaring-virtual-packages-per-platform) share configuration, you can match them with a `*` wildcard in the target selector instead of repeating each block.
The pattern is matched against the platform *name*, so it is most useful together with custom platform names:

```toml title="pixi.toml"
[workspace]
platforms = [
  { name = "cuda-win-64", platform = "win-64", cuda = "12" },
  { name = "cuda-linux-64", platform = "linux-64", cuda = "12" },
  "win-64",
  "linux-64",
]

[target."cuda-*".tasks]
test = "python test.py --cuda"
train = "python train.py --cuda"
```

Here both `cuda-win-64` and `cuda-linux-64` pick up the `test` and `train` tasks, while the bare `win-64` and `linux-64` platforms do not.

A few details:

- `*` is the only metacharacter and matches any run of characters. Patterns are matched in full and are case-sensitive (`cuda-*`, `*-64`, `*cuda*`).
- When more than one selector matches a platform, the one defined **later** in the manifest wins, the same way `[target.linux]` and `[target.linux-64]` already combine. Place a specific `[target.cuda-win-64]` override *after* the `[target."cuda-*"]` block.
- Wildcards are only allowed on workspace and feature targets. They are rejected in `[package.target]` and `[package.build.target]`, which resolve by subdir.

## Declaring virtual packages per platform

A bare-string entry like `"linux-64"` is shorthand for "the conda subdir `linux-64` with Pixi's [default declared virtual packages](#default-declared-virtual-packages)".
An inline table declares additional [virtual packages](https://docs.conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html) for the solve, for example a CUDA driver capability or a glibc version.

!!! info "Replaces `[system-requirements]`"
    These inline-table entries are the recommended way to declare CUDA, glibc, macOS, archspec, and similar constraints. The older `[system-requirements]` table still parses but is deprecated; see [Migrating from `[system-requirements]`](./system_requirements.md) for the equivalent forms.

```toml title="pixi.toml"
[workspace]
platforms = [
  "osx-arm64",
  { platform = "linux-64", cuda = "12.0", glibc = "2.28" },
  { name = "jetson-nano", platform = "linux-aarch64", cuda = "12.8" },
]
```

Each inline-table entry has:

- `platform`: the conda subdir the entry targets (e.g. `linux-64`, `osx-arm64`). Required.
- `name`: optional workspace-scoped identifier the platform is referenced by elsewhere (in target selectors, in `feature.<name>.platforms`, in lock file rows, in CLI commands).
  When omitted, Pixi synthesizes a name from `platform` plus the declared virtual packages, so two entries that declare the same set in different key order share the same identifier.
- Friendly keys for the common virtual packages: `cuda`, `amdgpu`, `archspec`, `glibc`, `linux`, `macos` (alias `osx`), `windows`.
  Each maps onto the matching `__name` conda virtual package (`cuda` -> `__cuda`, `amdgpu` -> `__amdgpu`, `glibc` -> `__glibc`, `macos` -> `__osx`, etc.).
- `archspec` names a CPU microarchitecture (`x86_64_v3`, `skylake`, `m1`,
  `armv8.2a`, ...) rather than a version. The name must be one the bundled
  [archspec](https://github.com/archspec/archspec) database knows, and Pixi
  rejects anything else with a suggestion: a name no host can ever report would
  otherwise silently produce a platform that never matches. Conda build strings
  cannot contain `-`, so the spelling uses underscores and dots (`x86_64_v3`,
  not `x86-64-v3`). A CPU newer than the bundled database can't be named until
  Pixi ships an updated archspec; set `archspec = "0"` to declare the
  microarchitecture explicitly unknown.
  Unlike many other virtual packages, a machine does not satisfy `archspec` by
  carrying a high enough version.
  It satisfies it by being that microarchitecture or one the archspec database
  says is a strict superset of it, so a `zen2` host covers a platform declaring
  `x86_64_v3` but a `haswell` host does not cover one declaring `zen2`.
  A machine whose own microarchitecture is unknown is accepted, since Pixi can
  neither prove nor disprove that it covers the requirement.
- `cuda` also accepts a `{ driver, arch }` table that declares the CUDA driver
  version (`__cuda`) together with the GPU compute capability (`__cuda_arch`):

  ```toml title="pixi.toml"
  platforms = [
    { name = "gpu", platform = "linux-64", cuda = { driver = "12.0", arch = "8.6" } },
  ]
  ```

  `driver` is exactly equivalent to the bare `cuda = "12.0"` form. Per the
  conda CEP, `__cuda_arch` is meaningless without `__cuda`, so `arch` requires
  `driver`; declaring `arch` (or a raw `__cuda_arch`) alone is rejected.

- `amdgpu = true` declares `__amdgpu`, which only states that an AMD GPU is
  present. An AMDGPU target name (`gfx90a`, `gfx1100`, ...) additionally
  declares the GPU architecture as `__amdgpu_arch`, which conda stores as an
  ISA version (`gfx90a` is `9.0.10`):

  ```toml title="pixi.toml"
  platforms = [
    { name = "rocm", platform = "linux-64", amdgpu = true },
    { name = "mi250", platform = "linux-64", amdgpu = "gfx90a" },
  ]
  ```

  A raw `__amdgpu_arch` takes the ISA version and is rejected without
  `__amdgpu`.
  AMD GPU ISAs are not generally backward compatible.
  Packages targeting a specific ISA must require it exactly, for example
  `__amdgpu_arch ==9.0.10` for `gfx90a`.
  Pixi compares the declaration as a minimum version when ordering candidates;
  the locked package requirement enforces the exact match.

- For virtual packages without a friendly key, use a raw `__name = "version"` entry, such as `__username_length = "3"` in the [detector tutorial](../advanced/virtual_package_detectors.md).
  The value is a concrete version, not a dependency expression such as `">=3"`.
  Use `"version=build_string"` to specify a build; the default build is `0`.
  Custom host capabilities come from a registered detector or a recognized [host override](#environment-variable-overrides), not from declaring the name in the manifest.

A feature's `platforms` array is a list of names that must each resolve to a workspace platform (or be a bare conda subdir, which Pixi treats as an alias for that subdir).
This is how you bind a feature to the rich variant:

```toml title="pixi.toml"
[workspace]
platforms = [
  "osx-arm64",
  { platform = "linux-64", cuda = "12.0" },
]

[feature.gpu]
platforms = ["linux-64-cuda-12-0"]  # the synthesized name for the entry above
```

!!! note "Platform names in `pixi.lock`"
    Platforms are written to `pixi.lock` under their manifest names, explicit or synthesized.
    An unambiguous name-only change can reuse the existing resolution when the target definition and effective dependencies stay the same.
    Pixi matches platforms by subdir and declared virtual packages, and writes the new names the next time a command may update the lock file.
    Changing a target selector can change dependencies, so a rename is not always resolution-free.

### Adding the current machine

`--auto-detect` writes the subdir and the virtual packages detected on the current machine (macOS version, glibc, archspec, CUDA, ...) into a platform entry:

```shell
pixi workspace platform add --auto-detect
```

This captures one known machine, which is usually more specific than your packages need.
The entry is inserted **first** in `platforms`, giving it priority within its [subdir](#which-platform-gets-installed).
It is a normal entry in the manifest, so everyone using the workspace solves for it, and machines that don't match it fall back to later entries of the same subdir.

```shell
# Give the detected platform a custom name instead of the synthesized one.
pixi workspace platform add my-laptop --auto-detect

# Override individual virtual packages on top of what was detected.
pixi workspace platform add --auto-detect --cuda 12.4
```

Pixi deduplicates by definition (subdir plus declared virtual packages), not by name: if an entry with the same definition already exists it is reused and moved to the front rather than duplicated.
Adding a platform whose definition already exists under a *different* name is rejected: two names for one definition would only produce a redundant duplicate solve.

!!! tip "Trim it for portability"
    After installing, `pixi info` reports each environment's **Minimum platform**, derived from locked conda package requirements.
    Use it to review declarations, not as proof that a requirement is safe to remove: the locked-requirement check excludes `__archspec` and does not establish that every binary can run.
    A future resolution can also select packages with different requirements.

## Managing platforms from the CLI

[`pixi workspace platform`](../reference/cli/pixi/workspace/platform/index.md) is the CLI surface for these entries:

- `pixi workspace platform add <PLATFORM> [--cuda 12.0] [--cuda-arch 8.6] [--amdgpu] [--amdgpu-arch gfx90a] [--glibc 2.28] ...`
  appends platforms with default or custom virtual packages (or detects the current machine via
  `--auto-detect`, see above). `--cuda-arch` requires `--cuda` (or
  an existing `__cuda`) and serializes as `cuda = { driver, arch }`.
  `--amdgpu-arch` also declares `__amdgpu` and serializes as `amdgpu = "gfx90a"`.
  Other virtual packages use `__name[=version[=build_string]]` positionals, for example `__site_service=2=h1`.
- `pixi workspace platform edit <NAME> [--cuda 12.1] [--remove-virtual-package __glibc]` edits a platform's declared virtual packages.
- `pixi workspace platform move <NAME> --to-top | --to-bottom | --before <NAME> | --after <NAME>` reorders an entry; order only matters among entries of the same subdir.
- `pixi workspace platform list` inspects what is declared.
- `pixi workspace platform remove <NAME>` drops an entry.

The mutating subcommands keep `pixi.lock` in sync.

## Which platform gets installed?

For a fresh automatic installation, Pixi tries the environment's platforms in two groups:

1. Platforms whose built-in virtual package declarations match the machine.
2. All remaining platforms in subdirs the machine can run.

Within each group, native subdirs come before supported fallbacks such as `osx-64` on `osx-arm64`.
Declaration order applies within a subdir.
For each candidate, Pixi checks the locked requirements and selects the first one that fits.
If a candidate fails, Pixi moves on to the next one.
If none fit, installation fails.

The declaration comparison only covers names Pixi defines:
`__unix`, `__linux`, `__osx`, `__win`, `__ios`, `__android`, `__glibc`, `__musl`, `__eglibc`, `__cuda`, `__cuda_arch`, `__amdgpu`, `__amdgpu_arch`, and `__archspec`.
This applies even when a channel detector supplies their values.
Except for [`__archspec`](#declaring-virtual-packages-per-platform), the detected version must be at least the declared version, with the same name and build string.
Declarations equal to Pixi's [subdir defaults](#default-declared-virtual-packages) are excluded from this comparison.
Version `0` is a concrete version, not a wildcard.
Diagnostics distinguish declared values such as `__cuda=0` from locked predicates such as `__cuda ==0`.

The locked-requirement check covers:

- Conda virtual package `depends`: the machine must provide a value matching the full predicate, including upper bounds and exact versions.
- Conda virtual package `constrains`: the predicate applies only if the machine provides that virtual package.
- PyPI wheel tags: the locked wheels must be compatible with the machine.

The conda checks exclude `__archspec`; its declaration comparison uses the archspec database instead.
Without a lock file, Pixi only uses the declaration comparison for preselection.

For example, a machine reporting CUDA `13.0` matches a `cuda = "12.0"` declaration.
If that platform's locked packages require `__cuda <13`, Pixi skips it and tries the next candidate, such as a CPU platform.
Skipped declaration-matching platforms and their unmet requirements appear in info-level logs.
`pixi info` lists every skipped platform.

Pixi permits a platform from the second group despite its declarations, and `pixi run` warns once.
For example, a platform declaring `cuda = "12.8"` can still install on CUDA `12.6` if its locked packages only require `__cuda >=12` and no declaration-matching platform fits.
A future resolution can pick packages that need the declared `12.8`.

Wheel tags describe Python, ABI, operating-system, and CPU-architecture compatibility, not CUDA driver or GPU requirements.
For GPU-enabled wheels without a locked `__cuda` requirement, a CUDA declaration therefore affects preference but does not prevent fallback to that platform on a machine without CUDA.

!!! warning "Custom declarations need locked requirements"
    Custom names such as `__username_length` are only checked through locked package requirements, not by comparing declarations.
    If two platforms differ only in a custom declaration and their locked requirements both fit, the first one wins regardless of the declared values.
    If no locked package requires the custom capability, selection cannot distinguish the platforms.
    Virtual packages listed directly as manifest dependencies are not recorded as package requirements in `pixi.lock`.

### Installed environments and explicit selection

`pixi install` selects a platform as described above.
If no declaration-matching platform has compatible locked packages and the environment is already installed for a still-declared platform, it keeps that platform instead of selecting one from the second group.
`pixi run` keeps the platform an environment was installed for, even when it updates the prefix after a lock file change.
The recorded platform name distinguishes targets with identical virtual-package declarations but different dependencies or activation settings.
For older environments without a recorded name, Pixi reuses the platform only if its definition identifies a single declaration; otherwise it selects again using the lock file.
It skips detection only when the installed prefix is still up to date.
Pixi does not monitor drivers or other host capabilities for changes.
See [detector consent and caching](../reference/pixi_configuration.md#virtual-package-detectors) for the validation boundary.

`pixi install --platform <NAME>` and `pixi run --platform <NAME>` select a declared platform explicitly and skip the host requirement checks.
They can install an environment for another machine; success does not prove that the environment runs locally.
Later `pixi run` calls without `--platform` keep that platform but check its locked requirements against the machine again, so they fail if the machine can't run it.
`PIXI_OVERRIDE_PLATFORM` changes the host subdir Pixi considers.
A foreign override skips the locked requirement checks, but declarations such as `cuda` must still be provided by the machine or a `CONDA_OVERRIDE_*` variable.
Neither option provides the target's hardware or system libraries.

`--locked` and `--frozen` control lock file freshness, not these host checks.
See [lock file satisfiability](./lock_file.md#lock-file-satisfiability).

## Default declared virtual packages

Bare subdir entries and inline-table entries both include subdir defaults.
Explicit values take precedence by virtual-package name.
The version floors are:

| Subdir | Version floors |
|---|---|
| Linux, except `linux-riscv64` | `__linux = "4.18"`, `__glibc = "2.28"` |
| `linux-riscv64` | `__linux = "4.18"`, `__glibc = "2.39"` |
| `osx-64`, `osx-arm64` | `__osx = "13.0"` |

This table is not the complete virtual-package set.
Unix subdirs also include `__unix`, Windows includes `__win`, and an `__archspec` baseline is included where available.
CUDA is not a default.
Declaring `__musl` or `__eglibc` replaces the default `__glibc` rather than adding a second libc family.

## Environment-variable overrides

`CONDA_OVERRIDE_*` variables change Pixi's view of the host.
They do not change the virtual packages supplied to a declared workspace solve.
To change the solve target, edit its platform declaration.

Overrides are useful for deliberately mocking host capabilities, for example to see which dependencies a CUDA machine would get:

```nushell
with-env { CONDA_OVERRIDE_CUDA: "12.0" } {
    pixi info
}
```

This reports a claimed CUDA capability, and the `Dependencies` of each environment come from the platform such a machine would select.
It does not install a driver or make a GPU-dependent application runnable.

| Override | Host capability |
|---|---|
| `CONDA_OVERRIDE_CUDA` | `__cuda` driver capability, for example `12` |
| `CONDA_OVERRIDE_CUDA_ARCH` | `__cuda_arch` compute capability, for example `8.6`; requires CUDA presence |
| `CONDA_OVERRIDE_AMDGPU` | `__amdgpu` presence; the only valid version is `0` |
| `CONDA_OVERRIDE_AMDGPU_ARCH` | `__amdgpu_arch` ISA version, for example `11.0.0`; requires AMDGPU presence |
| `CONDA_OVERRIDE_GLIBC` | `__glibc` version, for example `2.28` |
| `CONDA_OVERRIDE_OSX` | `__osx` version, for example `13.0` |
| `CONDA_OVERRIDE_LINUX` | `__linux` version, for example `4.18` |
| `CONDA_OVERRIDE_WIN` | `__win` version, for example `0` |
| `CONDA_OVERRIDE_ARCHSPEC` | CPU microarchitecture, for example `skylake`; `0` marks it unknown |

For example, mocking an AMD GPU architecture requires both `CONDA_OVERRIDE_AMDGPU=0` and `CONDA_OVERRIDE_AMDGPU_ARCH=11.0.0`.
Built-in overrides have platform-specific parsing rules; an OS override does not turn the host into that OS.
An empty value disables the corresponding capability in the host view, not in the target declaration.

For custom detector names, accepted channel registrations establish the override names.
For example, `__username_length` uses `CONDA_OVERRIDE_USERNAME_LENGTH`, with a value such as `"9"` or `"9=0"` to include a build string.
An override can supply that host value without executing or trusting the detector.
Arbitrary `CONDA_OVERRIDE_*` variables are not automatically imported as capabilities without a recognized name.
User-supplied override variables can be inherited by child processes like other environment variables; this is not detector-result injection.

## Virtual package detectors

Channels can register virtual package detectors: packages that report system capabilities, such as an MPI runtime or a driver installed outside the environment.
Pixi compares built-in names with declarations and checks all names against locked package requirements.
Custom names are not compared with declarations; see the [selection rules and custom-name limitation](#which-platform-gets-installed).
Detectors can extend or replace Pixi's built-in detection.

Pixi runs detectors when their virtual packages are needed for the current machine, not for foreign target platforms.
`pixi lock` can run them too, to select the platform for the current machine, but workspace solves only use the virtual packages declared in the manifest.

Detectors execute code from the channel, so Pixi asks whether to trust them before running them.
You can save the decision in the [configuration](../reference/pixi_configuration.md#virtual-package-detectors).
Without a terminal, detectors without a saved decision are skipped.
Detector report caching and installed-prefix reuse are separate; see the [consent and cache policy](../reference/pixi_configuration.md#virtual-package-detectors).

See [Channel-provided virtual package detectors](../advanced/virtual_package_detectors.md) for a complete example with a Rust detector, a local channel, and a package depending on its result.
