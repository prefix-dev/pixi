# Running Pixi on Untrusted Repositories

Pixi can run code that a repository chooses, even in commands that only update the lock file.
That is a problem wherever Pixi runs automatically on repositories whose content the operator of the machine does not control.
This section is for those setups: dependency update bots, and CI jobs that run on pull requests from forks.

**Dependency update bots.**
A bot such as Renovate usually runs on one runner and serves many repositories of an organization.
For each repository it checks out the code, runs `pixi lock` or `pixi update`, and opens a pull request with the new lock file.
To do that, the runner holds credentials that are more powerful than those of most people in the organization: a token that can push to every repository the bot serves, and often the credentials for private channels and package indexes.

Without further measures, everyone who can get a change into the manifest of one of these repositories can run arbitrary code on that runner.
It is enough to add a source dependency whose `[package.build.backend]` names a package and a channel of their choice: Pixi installs that package and starts it while it solves.
The code then runs with the bot's credentials, so write access to one repository turns into write access to all of them.
Being allowed to change one repository should not give anyone code execution on the runner that updates every repository.

**Pull requests from forks.**
The same applies to a CI job that runs Pixi on pull requests from forks while it has access to secrets, for example to check that the lock file is up to date.
Here the author does not even have to be a member of the organization.

For these setups, Pixi can be told not to run any code that the operator has not explicitly trusted, by disabling untrusted code execution:

```shell
pixi lock --no-untrusted-code-exec
```

With untrusted code execution disabled, Pixi still reads the manifest, solves the environments and writes `pixi.lock`, but it refuses everything that would install an environment or run code from the repository.
The only code it runs is the [build backends](../build/backends.md) that you list in your [trusted packages](#trusting-build-backends), because a workspace with source dependencies cannot be solved without them.

## Disabling untrusted code execution

There are three ways to disable untrusted code execution, and they are applied in this order:

1. The `--no-untrusted-code-exec` flag. It is accepted by every command, before or after the command name. `--no-untrusted-code-exec=false` enables untrusted code execution again for this one call.
2. The `PIXI_NO_UNTRUSTED_CODE_EXEC` environment variable, set to a boolish value such as `1` or `true`. Any other value, including an empty one, is an error.
3. The [`security.untrusted-code-exec`](../reference/pixi_configuration.md#security) configuration key, set to `false` in a system or user configuration file.

```toml title="~/.pixi/config.toml"
[security]
untrusted-code-exec = false
```

Untrusted code execution is enabled by default, which is the behavior Pixi always had.

!!! warning "The repository cannot change this setting"
    Pixi ignores the whole `[security]` table in a workspace's own `.pixi/config.toml`, and warns about it.
    A repository can neither disable untrusted code execution, nor enable it, nor add trusted packages.
    The setting has to come from outside the repository, because a change that adds malicious code would simply remove such a line in the same commit.

`pixi info` shows whether untrusted code execution is disabled for that invocation and where the setting came from.

## What still works

With untrusted code execution disabled, commands fall into three groups.

**Commands that work as usual**, because they only read or write files and solve:
`pixi lock`, `pixi update`, `pixi init`, `pixi import`, `pixi task`, `pixi config`, `pixi auth`, `pixi search`, `pixi info`, `pixi clean`, `pixi upload`, `pixi self-update`, `pixi global list`, `pixi global tree`, and the `pixi workspace` subcommands not listed below.

**Commands that need `--no-install`**, because they would install an environment as a side effect:
`pixi add`, `pixi remove`, `pixi upgrade`, `pixi list`, `pixi tree`, `pixi workspace channel add/remove`, `pixi workspace platform add/remove`, and `pixi workspace export conda-explicit-spec`.
Without `--no-install` they fail before doing anything, even when this particular call would not have installed anything.
That way a command either always works or always fails for a given setup, which is what a bot needs.

```shell
pixi add --no-install --no-untrusted-code-exec numpy
```

**Commands that are refused**, because installing or running code is what they are for:
`pixi install`, `pixi reinstall`, `pixi run`, `pixi shell`, `pixi shell-hook`, `pixi exec`, `pixi build`, `pixi publish`, and the `pixi global` commands that install or remove packages.

These lists may get shorter in future releases, for example by allowing installs that do not run any scripts.
That would only make more commands work; nothing that works today will start to fail.

!!! note "PyPI dependencies that need a build"
    Some PyPI packages only reveal their dependencies after they have been built, for example source distributions without static metadata, and some git or path dependencies.
    Building them requires installing the conda environment first, which is refused.
    A workspace with such dependencies cannot be locked while untrusted code execution is disabled.

!!! note "Git dependencies"
    While untrusted code execution is disabled, Pixi only fetches git repositories over `https` and `ssh`.
    This applies to git source dependencies and to their submodules.
    Other git transports are refused, whatever the git configuration of the machine says, because some of them can run a command that is given in the URL.

## Trusting build backends

A workspace with [source dependencies](../build/getting_started.md) cannot be solved without its build backend: Pixi installs the backend into its own environment and asks it for the package metadata.
The repository chooses that backend, its version and its channels, so with untrusted code execution disabled Pixi only installs and runs a backend environment if **every** package in it is listed in `security.trusted-packages`.
Workspaces without source dependencies do not need any trusted packages.

The list is a set of rules.
Each rule names one channel, by its full URL, and the packages you trust from that channel:

```toml title="~/.pixi/config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:security"
```

A package in a backend environment is trusted if at least one rule matches it.
A rule matches when all of the following hold:

- **Name:** the package name is listed in the rule's `packages`. There are no wildcards.
- **Version:** the package version matches the entry's `version`, a version spec like `"*"`, `"0.8.*"` or `">=0.8.6,<0.9"`.
- **Channel:** the package comes from exactly the rule's `channel`.
  URLs are compared by scheme, host, port and path, so `https://prefix.dev/` does not cover `https://prefix.dev/conda-forge`, and `https://conda.anaconda.org/conda-forge/label/rc` is a different channel from `https://conda.anaconda.org/conda-forge`.
  Channel names like `conda-forge` are not accepted, because what a name expands to can be changed by configuration.
  The channel URLs a workspace uses are listed at the top of its `pixi.lock`.
- **Checksum:** the package has a `sha256` in the channel's repodata, so Pixi can verify the file it downloads.
- **Age:** if the rule has `reject-newer`, the package was published before that cutoff.
  `reject-newer` takes a date, a timestamp, or a duration like `"7d"`, as [`exclude-newer`](../reference/pixi_manifest.md#exclude-newer-optional) does.
  A package without a timestamp does not match a rule with `reject-newer`.

!!! warning "`reject-newer` does not influence the solve"
    `reject-newer` only decides whether a package is trusted, after Pixi has solved the backend environment.
    It does not make the solver choose an older package.
    When a package of the backend environment gets a new release, the solver selects it, the check rejects it as too new, and the command fails until the release is older than the cutoff.

    To avoid this, the repository's manifest needs an [`exclude-newer`](../reference/pixi_manifest.md#exclude-newer-optional) of at least the same duration, so that the solver skips the packages the check would reject.
    The `exclude-newer` of the Pixi configuration does not help here, because it never applies to an existing workspace.

Every package in the backend environment must be listed, not only the backend itself.
This includes its runtime dependencies, such as `openssl` or the C runtime on Windows, and anything the repository adds with [`additional-dependencies`](../build/backends.md).
The packages differ by platform, so a list that works on Linux may be incomplete on Windows.

When a backend environment contains a package that is not trusted, Pixi neither installs it nor reuses it from its cache, and fails with an error that lists every such package and why it did not match.

Trusted packages have no effect while untrusted code execution is enabled.

!!! note "Mirrors"
    If you configure a [mirror](../reference/pixi_configuration.md#mirrors) for a channel, the rule must name the mirror's URL, because that is where the package actually comes from.
    If a channel has several mirrors, every one of them must be trusted, because Pixi may use any of them for a request.
    Mirrors defined in the repository's `.pixi/config.toml` are ignored while untrusted code execution is disabled.

!!! note "Backends chosen by Pixi"
    Some source dependencies do not name a backend themselves.
    A source that only contains a `recipe.yaml` uses `pixi-build-rattler-build`, and ROS packages use `pixi-build-ros`.
    These backends need trusted packages too.

## The repository's configuration file

A repository can ship its own `.pixi/config.toml`.
While untrusted code execution is disabled, Pixi only applies the settings from that file that cannot change which code runs or where packages come from, such as `default-channels`, `pinning-strategy` or `concurrency`.
It ignores, with a warning naming each one, settings like `mirrors`, `proxy-config`, `tls-no-verify`, `tls-root-certs`, `s3-options`, `authentication-override-file`, `cache`, `detached-environments` and `run-post-link-scripts`.

## What this does not protect against

Disabling untrusted code execution is about which code Pixi runs on the machine it is called on.
It does not make the packages in the lock file safe: the lock file Pixi writes can still contain malicious packages, which run as soon as someone installs the environment.
Review lock file changes as described in [Supply Chain Security](../security.md#1-make-dependency-resolution-reproducible).

The protection also depends on assumptions about software that Pixi does not control:

- **Trusted backends do not run repository code while they report metadata.**
  Pixi asks a backend for package metadata only and never asks it to build.
  A backend that executes code from the repository at that stage, or lets the repository's configuration enable such code, breaks the protection.
  By trusting a backend you rely on it to behave this way.
- **The package cache was not written by a run on an untrusted repository with untrusted code execution enabled.**
  Pixi verifies packages against their checksum, but a run that executed malicious code could have changed the cache.
  On machines that work on both trusted and untrusted repositories, give the untrusted runs their own cache with `PIXI_CACHE_DIR`.
