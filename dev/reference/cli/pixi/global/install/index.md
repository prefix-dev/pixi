# `[pixi](../../) [global](../) install`

## About

Installs the defined packages in a globally accessible location and exposes their command line applications.

Tip

Running `osx-64` on Apple Silicon will install the Intel binary but run it using [Rosetta](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment)

```text
pixi global install --platform osx-64 ruff

```

After using global install, you can use the package you installed anywhere on your system.

## Usage

```text
pixi global install [OPTIONS] <PACKAGE>...

```

## Arguments

- [`<PACKAGE>`](#arg-%3CPACKAGE%3E) The dependency as names, conda MatchSpecs

  May be provided more than once.

  **required**: `true`

## Options

- [`--channel (-c) <CHANNEL>`](#arg---channel) The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times

  May be provided more than once.

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform to install the packages for

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) Ensures that all packages will be installed in the same environment

- [`--expose <EXPOSE>`](#arg---expose) Add one or more mapping which describe which executables are exposed. The syntax is `exposed_name=executable_name`, so for example `python3.10=python`. Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed

  May be provided more than once.

- [`--with <WITH>`](#arg---with) Add additional dependencies to the environment. Their executables will not be exposed

  May be provided more than once.

- [`--force-reinstall`](#arg---force-reinstall) Specifies that the environment should be reinstalled

- [`--no-shortcuts`](#arg---no-shortcuts) Specifies that no shortcuts should be created for the installed packages

## Config Options

- [`--auth-file <AUTH_FILE>`](#arg---auth-file) Path to the file containing the authentication token

- [`--concurrent-downloads <CONCURRENT_DOWNLOADS>`](#arg---concurrent-downloads) Max concurrent network requests, default is `50`

- [`--concurrent-solves <CONCURRENT_SOLVES>`](#arg---concurrent-solves) Max concurrent solves, default is the number of CPUs

- [`--pinning-strategy <PINNING_STRATEGY>`](#arg---pinning-strategy) Set pinning strategy

  **options**: `semver`, `minor`, `major`, `latest-up`, `exact-version`, `no-pin`

- [`--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>`](#arg---pypi-keyring-provider) Specifies whether to use the keyring to look up credentials for PyPI

  **options**: `disabled`, `subprocess`

- [`--run-post-link-scripts`](#arg---run-post-link-scripts) Run post-link scripts (insecure)

- [`--tls-no-verify`](#arg---tls-no-verify) Do not verify the TLS certificate of the server

- [`--use-environment-activation-cache`](#arg---use-environment-activation-cache) Use environment activation cache (experimental)

## Description

Installs the defined packages in a globally accessible location and exposes their command line applications.

Example:

- `pixi global install starship nushell ripgrep bat`
- `pixi global install jupyter --with polars`
- `pixi global install --expose python3.8=python python=3.8`
- `pixi global install --environment science --expose jupyter --expose ipython jupyter ipython polars`

## Examples

```shell
pixi global install ruff
# Multiple packages can be installed at once
pixi global install starship rattler-build
# Specify the channel(s)
pixi global install --channel conda-forge --channel bioconda trackplot
# Or in a more concise form
pixi global install -c conda-forge -c bioconda trackplot
# Support full conda matchspec
pixi global install python=3.9.*
pixi global install "python [version='3.11.0', build_number=1]"
pixi global install "python [version='3.11.0', build=he550d4f_1_cpython]"
pixi global install python=3.11.0=h10a6764_1_cpython
# Install for a specific platform, only useful on osx-arm64
pixi global install --platform osx-64 ruff
# Install a package with all its executables exposed, together with additional packages that don't expose anything
pixi global install ipython --with numpy --with scipy
# Install into a specific environment name and expose all executables
pixi global install --environment data-science ipython jupyterlab numpy matplotlib
# Expose the binary under a different name
pixi global install --expose "py39=python3.9" "python=3.9.*"

```
