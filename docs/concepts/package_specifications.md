# Package Specifications

When adding packages to your Pixi workspace or global environment, you can use various specifications to control exactly which package version and build you want.
This is particularly important when packages have multiple builds for different hardware configurations (like CPU vs GPU).

## Quick Examples

Here are common use cases:

=== "Add"
    ```bash
    --8<-- "docs/source_files/shell/package_specifications.sh:quick-add-examples"
    ```
=== "Global"
    ```bash
    --8<-- "docs/source_files/shell/package_specifications.sh:quick-global-examples"
    ```
=== "Exec"
    ```bash
    --8<-- "docs/source_files/shell/package_specifications.sh:quick-exec-examples"
    ```

## Basic Version Specifications
Pixi uses the **conda MatchSpec** format for specifying package requirements.
A MatchSpec allows you to precisely define which package version, build, and channel you want.

The simplest way to specify a package is by name and [optional version operators](#version-operators):

```toml
--8<-- "docs/source_files/pixi_tomls/package_specifications.toml:basic-version"
```

## Full MatchSpec Syntax

Beyond simple version specifications, you can use the full MatchSpec syntax to precisely control which package variant you want.

### Command Line Syntax

Pixi supports two syntaxes on the command line:

**1. Equals syntax (compact):**
```shell
# Format: package=version=build
pixi add "pytorch=2.0.*=cuda*"
# Only build string (any version)
pixi add "numpy=*=py311*"
```

**2. Bracket syntax (explicit):**
```shell
# Format: package [key='value', ...]
pixi add "pytorch [version='2.0.*', build='cuda*']"
# Multiple constraints
pixi add "numpy [version='>=1.21', build='py311*', channel='conda-forge']"
# Build number constraint
pixi add "python [version='3.11.0', build_number='>=1']"
```

Both syntaxes are equivalent and can be used interchangeably.
Choose based on your preference:

- Use **equals syntax** for quick, compact specifications
- Use **bracket syntax** for better readability and when combining multiple constraints

### TOML Mapping Syntax

In your `pixi.toml`, use the mapping syntax for complete control:

```toml
--8<-- "docs/source_files/pixi_tomls/package_specifications.toml:mapping-syntax-full"
```

This syntax allows you to specify:

- **version**: Version constraint using [operators](#version-operators)
- **build**: Build string pattern (see [build strings](#build-strings))
- **build-number**: Build number constraint (e.g., `">=1"`, `"0"`) (see [build number](#build-number))
- **channel**: Specific channel name or full URL (see [channel](#channel))
- **sha256/md5**: Package checksums for verification (see [checksums](#checksums-sha256md5))
- **license**: Expected license type (see [license](#license))
- **file-name**: Specific package file name (see [file name](#file-name))

### Version Operators

Pixi supports various version operators:

| Operator | Meaning | Example |
|----------|---------|---------|
| `==`     | Exact match | `==3.11.0` |
| `!=`     | Not equal | `!=3.8` |
| `<`      | Less than | `<3.12` |
| `<=`     | Less than or equal | `<=3.11` |
| `>`      | Greater than | `>3.9` |
| `>=`     | Greater than or equal | `>=3.9` |
| `~=`     | Compatible release | `~=3.11.0` (>= 3.11.0, < 3.12.0) |
| `*`      | Wildcard | `3.11.*` (any 3.11.x) |
| `,`      | AND | `">=3.9,<3.12"` |
| `|`     | OR | `"3.10|3.11"` |

### Build Strings

Build strings identify specific builds of the same package version.
They're especially important for packages that have different builds for:

- **Hardware acceleration**: CPU, GPU/CUDA builds
- **Python versions**: Packages built for different Python interpreters
- **Compiler variants**: Different compiler versions or configurations

A build string typically looks like: `py311h43a39b2_0`

Breaking it down:

- `py311`: Python version indicator
- `h43a39b2`: Hash of the build configuration
- `_0`: Build number

You can use wildcards in build patterns to match multiple builds:

```shell
# Match any CUDA build
pixi add "pytorch=*=cuda*"

# Match Python 3.11 builds
pixi add "numpy=*=py311*"

# Using bracket syntax
pixi add "pytorch [build='cuda*']"
```

### Build Number

The build number is an integer that increments each time a package is rebuilt with the same version.
Use build number constraints when you need a specific rebuild of a package:

```shell
# Specific build number
pixi add "python [version='3.11.0', build_number='1']"

# Build number constraint
pixi add "numpy [build_number='>=5']"
```

**In pixi.toml:**
```toml
[dependencies.python]
version = "3.11.0"
build-number = ">=1"
```

Build numbers are useful when:

- A package was rebuilt to fix a compilation issue
- You need to ensure you have a specific rebuild with bug fixes
- Working with reproducible environments that require exact builds

### Channel

Channels are repositories where conda packages are hosted.
You can specify which channel to fetch a package from:

```shell
# Specific channel by name
pixi add "pytorch [channel='pytorch']"

# Channel URL
pixi add "custom-package [channel='https://prefix.dev/my-channel']"

# Or use the shorter `::` syntax
pixi add pytorch::pytorch
pixi add https://prefix.dev/my-channel::custom-package
```

**In pixi.toml:**
```toml
[dependencies.pytorch]
channel = "pytorch"

[dependencies.custom-package]
channel = "https://prefix.dev/my-channel"
```

Note that for `pixi add`, channels must also be listed in your workspace configuration:

```toml
[workspace]
channels = ["conda-forge", "pytorch", "nvidia"]
```

You can also add these using the command line:

```shell
pixi workspace channel add conda-forge
```

### Checksums (SHA256/MD5)

Checksums verify package integrity and authenticity.
Use them for reproducibility and security:

```toml
[dependencies.numpy]
version = "1.21.0"
sha256 = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
md5 = "abcdef1234567890abcdef1234567890"
```

When specified, Pixi will:

- Verify the downloaded package matches the checksum
- Fail the installation if checksums don't match
- Ensure you get the exact package you expect

SHA256 is preferred over MD5 for security reasons.

### License

Specify the expected license of a package.
This is useful for:

- Ensuring compliance with your organization's policies
- Filtering packages by license type
- Documentation purposes

```toml
[dependencies.pytorch]
version = "2.0.*"
license = "BSD-3-Clause"
```

### File Name

Specify the exact package file name to download.
This is rarely needed but useful for:

- Debugging package resolution issues
- Ensuring a specific package artifact
- Advanced use cases with custom package builds

```toml
[dependencies.pytorch]
file-name = "pytorch-2.0.0-cuda.tar.bz2"
```

## Source Packages

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    Please keep that in mind when you use it for your workspaces.

For these packages to be recognized they need to be understood by Pixi as source packages.
Look at the [Pixi Manifest Reference](../reference/pixi_manifest.md#the-package-section) for more information on how to declare source packages in your `pixi.toml`.

### Path-based Source Packages
```toml
--8<-- "docs/source_files/pixi_tomls/package_specifications.toml:path-fields"
```

The path should be relative to the workspace root or an absolute path, but absolute paths are discouraged for portability.


### Git-based Source Packages
```toml
--8<-- "docs/source_files/pixi_tomls/package_specifications.toml:git-fields"
```

For git repositories, you can specify:

- **git**: Repository URL
- **branch**: Git branch name
- **tag**: Git tag
- **rev**: Specific git revision/commit SHA
- **subdirectory**: Path within the repository

## PyPI package specifications

Pixi also supports installing packages from PyPI using `pixi add --pypi` and in your `pixi.toml` and `pyproject.toml`.

### Command Line Syntax

When using `pixi add --pypi`, you can specify packages similarly to pip:

```shell
# Simple package
pixi add --pypi requests

# Specific version
pixi add --pypi "requests==2.25.1"

# Version range
pixi add --pypi "requests>=2.20,<3.0"

# Extras
pixi add --pypi "requests[security]==2.25.1"

# URL
pixi add --pypi "requests @ https://files.pythonhosted.org/packages/1e/db/4254e3eabe8020b458f1a747140d32277ec7a271daf1d235b70dc0b4e6e3/requests-2.32.5-py3-none-any.whl#sha256=2462f94637a34fd532264295e186976db0f5d453d1cdd31473c85a6a161affb6"

# Git repository
pixi add --pypi "requests @ git+https://github.com/psf/requests.git@v2.25.1"
pixi add --pypi requests --git https://github.com/psf/requests.git --tag v2.25.1
pixi add --pypi requests --git https://github.com/psf/requests.git --branch main
pixi add --pypi requests --git https://github.com/psf/requests.git --rev 70298332899f25826e35e42f8d83425124f755a
```

### TOML Mapping Syntax
In your `pixi.toml` or `pyproject.toml` (under `[tool.pixi.pypi-dependencies]`), you can specify PyPI packages like this:

```toml
--8<-- "docs/source_files/pixi_tomls/package_specifications.toml:pypi-fields"
```

## Further Reading

- [Pixi Manifest Reference](../reference/pixi_manifest.md#dependencies) - Complete dependency specification options
- [Multi-Platform Configuration](../workspace/multi_platform_configuration.md) - Platform-specific dependencies
- [Conda Package Specification](https://conda.io/projects/conda/en/latest/user-guide/concepts/pkg-specs.html) - Conda's package specification docs
