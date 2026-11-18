A Pixi workspace can support several operating systems and, within one operating system, several hardware variants such as CPU-only and CUDA machines.
This page starts with a workspace for several operating systems, adds CPU and CUDA variants, and then covers the full selection rules.

## Supporting several operating systems

List the [conda subdirs](../conda_ecosystem.md#subdirs-how-conda-targets-platforms) your workspace supports in `workspace.platforms`:

```toml title="pixi.toml"
[workspace]
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]

[dependencies]
python = "3.14.*"

[tasks]
version = "python --version"
```

The environment is resolved separately for each platform, and `pixi.lock` stores one package set per platform.
Pixi resolves all of them on any machine, so a Linux machine also locks the macOS and Windows package sets.
`pixi install` and `pixi run` install the package set that fits the current machine.
[Target tables](../reference/pixi_manifest.md#the-target-table) add dependencies, tasks, or activation scripts for individual platforms.

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
Which packages fit is up to their run dependencies and constraints, for example `__cuda >=12` in `cuda-version`'s [`constrains`](https://rattler.prefix.dev/py-rattler/latest/package_record/#rattler.repo_data.package_record.PackageRecord.constrains).
Whether the current machine can run the resulting environment is checked when installing.

`pixi.lock` stores the declarations of each platform and the requirements of each package, never the detected virtual package values.
With a `linux-64-cuda` platform declaring `cuda = "12.0"` and a plain `linux-64-cpu` platform, the relevant parts look like this:

```yaml title="pixi.lock"
platforms:
- name: linux-64-cuda
  subdir: linux-64
  virtual-packages:
  - __cuda=12.0
  - __unix=0=0
  - __linux=4.18
  - __glibc=2.28
  # ...
- name: linux-64-cpu
  subdir: linux-64
  virtual-packages:
  - __unix=0=0
  - __linux=4.18
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

When installing `default` on a machine with CUDA 12.0 or newer, Pixi tries `linux-64-cuda` first and selects it if its locked packages fit.
Otherwise, and on machines without CUDA 12.0, it tries `linux-64-cpu`.
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

`pixi lock` resolves three package sets for the `default` environment, including the CUDA one, even when run on a machine without a GPU.

The CPU platform requires `pytorch-cpu` explicitly.
Leaving out the CUDA declaration only means that the solver can't use packages that require `__cuda`; it does not request a CPU build.

The manifest reference lists all [platform entry keys](../reference/pixi_manifest.md#inline-table-entries-per-platform-virtual-packages) and [target selectors](../reference/pixi_manifest.md#the-target-table).

`linux-64-cuda` is solved as if the machine had a driver for CUDA 12.0, and the run dependencies and constraints of the CUDA packages decide which builds fit that.

On a `linux-64` machine with CUDA 12.0 or newer, Pixi tries `linux-64-cuda` first because it appears first among the matching `linux-64` platforms in `workspace.platforms`.
If its locked packages don't fit the machine, Pixi tries `linux-64-cpu` instead.
Machines without a matching CUDA declaration try the CPU platform first.
Order matters: if `linux-64-cpu` came first and its locked packages fit, the CUDA platform would never be selected automatically.
`pixi run` keeps using an installed environment with its platform, so installing a driver later doesn't switch an existing CPU environment to CUDA.
Run `pixi install` to select again.
[Which platform gets installed?](#which-platform-gets-installed) has the complete rules.

The [PyTorch guide](../python/pytorch.md) covers pinning the CUDA version with `cuda-version` and installing PyTorch from PyPI.

## Adding the current machine

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

- `pixi workspace platform add <PLATFORM>` appends a platform with the subdir defaults.
  `pixi workspace platform add <NAME>=<PLATFORM> [--cuda 12.0] [--cuda-arch 8.6] [--amdgpu] [--amdgpu-arch gfx90a] [--glibc 2.28] ...`
  appends a named platform with custom virtual packages (or detects the current machine via
  `--auto-detect`, see above). `--cuda-arch` requires `--cuda` (or
  an existing `__cuda`) and serializes as `cuda = { driver, arch }`.
  `--amdgpu-arch` also declares `__amdgpu` and serializes as `amdgpu = "gfx90a"`.
  Other virtual packages use `__name[=version[=build_string]]` positionals, for example `__site_service=2=h1`.
- `pixi workspace platform edit <NAME> [--cuda 12.1] [--remove-virtual-package __cuda_arch]` edits a platform's declared virtual packages.
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
The CUDA 12.8/12.6 example below shows selection from the second group.

The declaration comparison only covers names Pixi defines:
`__unix`, `__linux`, `__osx`, `__win`, `__ios`, `__android`, `__glibc`, `__musl`, `__eglibc`, `__cuda`, `__cuda_arch`, `__amdgpu`, `__amdgpu_arch`, and `__archspec`.
This applies even when a channel detector supplies their values.
Except for [`__archspec`](../reference/pixi_manifest.md#inline-table-entries-per-platform-virtual-packages), the detected version must be at least the declared version, with the same name and build string; an empty build string counts as `0`.
Declarations equal to Pixi's [subdir defaults](#default-declared-virtual-packages) are excluded from this comparison.
Version `0` is a concrete version, not a wildcard.
Diagnostics distinguish declared values such as `__cuda=0` from locked predicates such as `__cuda ==0`.

The locked-requirement check covers:

- Conda virtual package `depends`: the machine must provide a value matching the full predicate, including upper bounds and exact versions.
- Conda virtual package `constrains`: the predicate applies only if the machine provides that virtual package.
- PyPI wheel tags: the locked wheels must be compatible with the machine.

The locked conda `depends` and `constrains` checks exclude `__archspec`: literal build names and regex predicates can reject compatible newer CPUs.
The separate declaration comparison uses the archspec database.
Operational commands resolve the lock file before selecting a platform.
`--no-install` skips prefix installation, not lock resolution: the default mode may update the lock, `--locked` rejects a missing or stale lock, and `--frozen` uses existing lock data without checking freshness.

For example, a machine reporting CUDA `13.0` matches a `cuda = "12.0"` declaration, since `13.0` compares `>= 12.0`.
However, if that platform's locked packages require `__cuda <13`, Pixi will skip it and try the next candidate (such as a CPU platform with no packages that require `__cuda`).
Declaration-matching platforms skipped for unmet requirements appear in info-level logs.
Diagnostic commands may not inspect the lock file; see [Diagnostic selection](#diagnostic-selection).

Pixi can permit a platform from the second group despite its declarations.
Activating it warns once per environment.
For example, a platform declaring `cuda = "12.8"` can still install on a machine with CUDA `12.6` if its locked packages only require `__cuda >=12` and no declaration-matching platform fits.
A future resolution may however pick packages that require `__cuda >=12.8`, at which point the environment will no longer be installable on that machine.

Wheel tags describe Python, ABI, operating-system, and CPU-architecture compatibility, not CUDA driver or GPU requirements.
For GPU-enabled wheels without a locked `__cuda` requirement, a CUDA declaration therefore affects preference but does not prevent fallback to that platform on a machine without CUDA.

!!! warning "Custom declarations need locked requirements"
    Custom names such as `__username_length` are only checked through locked package requirements, not by comparing declarations.
    If two platforms differ only in a custom declaration and their locked requirements both fit, the first one wins regardless of the declared values.
    If no locked package requires the custom capability, selection cannot distinguish the platforms.
    Virtual packages listed directly as manifest dependencies are not recorded as package requirements in `pixi.lock`.

### Installed environments and explicit selection

`pixi install` selects a platform as described above.
`pixi run` keeps the installed platform while reusing an up-to-date prefix.
If a lock change requires the prefix to be rebuilt, Pixi selects again; the rebuilt prefix, target-specific tasks, and activation all use that selection.
With `--no-install`, the prefix is not rebuilt and its installed platform is kept even if the lock changes; the command fails if the new lock lacks that platform or its requirements don't fit the machine.
The recorded platform name distinguishes targets with identical virtual-package declarations but different dependencies or activation settings.
For older environments without a recorded name, Pixi reuses the platform only if its definition identifies a single declaration; otherwise it selects again using the lock file.
Pixi skips detection when it reuses an installed platform.
Pixi does not monitor drivers or other host capabilities for changes.
See [detector consent and caching](../reference/pixi_configuration.md#virtual-package-detectors) for the validation boundary.

`pixi install --platform <NAME>` and `pixi run --platform <NAME>` select a declared platform explicitly and skip detection and the host requirement checks.
They can install an environment for another machine; success does not prove that the environment runs locally.
Later `pixi run` calls without `--platform` keep that platform while reusing the prefix but check its recorded package requirements against the machine, so they fail if the machine can't run it.
`PIXI_OVERRIDE_PLATFORM` changes the host subdir Pixi considers.
A foreign override skips the locked requirement checks and selects the first platform whose declarations match, without falling back to the second group.
For a subdir the machine can't run, declarations such as `cuda` must be provided by a `CONDA_OVERRIDE_*` variable.
Neither option provides the target's hardware or system libraries.

`--locked` and `--frozen` control lock file freshness, not these host checks.
See [lock file satisfiability](./lock_file.md#lock-file-satisfiability).

### Diagnostic selection

`pixi info`, `pixi task list`, and `pixi run` without a task never solve or update the lock file.
They reuse an installed platform without consulting the lock.
Otherwise, unless a dependency-free environment uses the host baseline, they try to read the existing lock and select against its requirements, even when a declaration matches the host.
Missing, unreadable, malformed, or newer-format locks leave the initial selection unchanged and do not prevent diagnostic output.
A lock file with no entry for a specified environment likewise leaves selection unchanged.
An existing lock is not checked for freshness.

Consequently, a diagnostic selection is opportunistic, not a promise about the next installation.
It can differ from the next installation if the lock is unavailable or stale, or installation updates it.
`pixi info` reports the selection source and, when it consults the lock file, skipped candidates.
An environment with no dependencies and no usable declared platform may still run host-only tasks; an environment with dependencies does not receive that fallback.

## Default declared virtual packages

Bare subdir entries and inline-table entries both include subdir defaults.
Explicit values take precedence by virtual-package name.
The version floors are:

| Subdir | Version floors |
|---|---|
| Linux, except `linux-riscv64` | `__linux = "4.18"`, `__glibc = "2.28"` |
| `linux-riscv64` | `__linux = "4.18"`, `__glibc = "2.39"` |
| `osx-64`, `osx-arm64` | `__osx = "13.0"` |
| `win-64`, `win-arm64` | `__win = "10.0"` |

This table is not the complete virtual-package set.
Unix subdirs also include `__unix` and an `__archspec` baseline is included where available.
CUDA is not a default.
Declaring `__musl` or `__eglibc` replaces the default `__glibc` rather than adding a second libc family.

## Environment-variable overrides

`CONDA_OVERRIDE_*` variables change what Pixi assumes about the current machine, for example to see which platform a CUDA machine would get:

```shell
CONDA_OVERRIDE_CUDA=12.0 pixi info
```

They don't change the virtual packages declared for a workspace solve, and they don't make GPU applications runnable.
An empty value removes that capability.

| Override | Virtual package |
|---|---|
| `CONDA_OVERRIDE_CUDA` | `__cuda`, for example `12` |
| `CONDA_OVERRIDE_CUDA_ARCH` | `__cuda_arch`, for example `8.6`; requires `__cuda` |
| `CONDA_OVERRIDE_AMDGPU` | `__amdgpu`; the only valid version is `0` |
| `CONDA_OVERRIDE_AMDGPU_ARCH` | `__amdgpu_arch`, for example `11.0.0`; requires `__amdgpu` |
| `CONDA_OVERRIDE_GLIBC` | `__glibc`, for example `2.28` |
| `CONDA_OVERRIDE_OSX` | `__osx`, for example `13.0` |
| `CONDA_OVERRIDE_LINUX` | `__linux`, for example `4.18` |
| `CONDA_OVERRIDE_WIN` | `__win`, the Windows OS version, for example `10.0` |
| `CONDA_OVERRIDE_ARCHSPEC` | `__archspec`, for example `skylake`; `0` marks it unknown |

Virtual packages from [detectors](#virtual-package-detectors) can be overridden the same way, for example `__username_length` with `CONDA_OVERRIDE_USERNAME_LENGTH=9`, without running the detector.

## Virtual package detectors

Channels can register virtual package detectors: packages that report capabilities of the current machine, such as an MPI runtime or a driver installed outside the environment.
In a workspace, their values are only used to [select a platform](#which-platform-gets-installed); workspace solves still use the virtual packages declared under `workspace.platforms`.
Detectors run code from the channel, so Pixi asks before trusting them; you can save that decision in the [configuration](../reference/pixi_configuration.md#virtual-package-detectors).

See [Channel-provided virtual package detectors](../advanced/virtual_package_detectors.md) for a complete example.
