# Package Specifications

When adding packages to your Pixi workspace or global environment, you can use various specifications to control exactly which package version and build you want. This is particularly important when packages have multiple builds for different hardware configurations (like CPU vs GPU). For the conda packages Pixi uses the [MatchSpec](https://rattler.prefix.dev/py-rattler/match_spec#matchspec) format to specify package requirements. For PyPI packages, Pixi uses the standard [PEP440 version specifiers](https://peps.python.org/pep-0440/).

## Quick Examples

```bash
# Install a specific version
pixi add python=3.11

# Install with version constraints
pixi add "numpy>=1.21,<2.0"

# Install a specific build (e.g., CUDA-enabled package) using = syntax
pixi add "pytorch=*=cuda*"

# Alternative bracket syntax for build specification
pixi add "pytorch [build='cuda*']"

# Specify both version and build using bracket syntax
pixi add "pytorch [version='2.9.*', build='cuda*']"

# Simple PyPI package
pixi add --pypi requests

# PyPI package version range
pixi add --pypi "requests>=2.20,<3.0"

# PyPI package with extras
pixi add --pypi "requests[security]==2.25.1"
```

```bash
# Install a specific version
pixi global install python=3.11

# Install with version constraints
pixi global install "numpy>=1.21,<2.0"

# Install a specific build (e.g., CUDA-enabled package) using = syntax
pixi global install "pytorch=*=cuda*"

# Alternative bracket syntax for build specification
pixi global install "pytorch [build='cuda*']"

# Specify both version and build using bracket syntax
pixi global install "pytorch [version='2.9.*', build='cuda*']"
```

```bash
# Execute a command in an ephemeral environment
pixi exec python

# Execute with specific package versions
pixi exec -s python=3.11 python

# Execute with specific package builds
pixi exec -s "python=*=*cp313" python

# Execute with channel specification
pixi exec --channel conda-forge python
```

## Basic Version Specifications

Pixi uses the [**conda MatchSpec**](https://rattler.prefix.dev/py-rattler/match_spec#matchspec) format for specifying package requirements. A MatchSpec allows you to precisely define which package version, build, and channel you want.

The simplest way to specify a package is by name and [optional version operators](#version-operators):

pixi.toml

```toml
[dependencies]
# Latest version (any)
numpy = "*"
# Specific version
python = "==3.11.0"
# Version range
scipy = ">=1.9,<2.0"
# Fuzzy version matching (any 3.11.x)
pandas = "3.11.*"
```

## Full MatchSpec Syntax

Beyond simple version specifications, you can use the full [MatchSpec](https://rattler.prefix.dev/py-rattler/match_spec#matchspec) syntax to precisely control which package variant you want.

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

### TOML Mapping Syntax

In your `pixi.toml`, use the mapping syntax for complete control:

pixi.toml

```toml
[dependencies.pytorch]
version = "2.0.*"
# Build string pattern
build = "cuda*"
# Build number constraint
build-number = ">=1"
# Specific channel
channel = "https://prefix.dev/my-channel"
# Checksums 
md5 = "1234567890abcdef1234567890abcdef"
sha256 = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
# License type
license = "BSD-3-Clause"
# Package file name
file-name = "pytorch-2.0.0-cuda.tar.bz2"
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

| Operator | Meaning               | Example                          |
| -------- | --------------------- | -------------------------------- |
| `==`     | Exact match           | `==3.11.0`                       |
| `!=`     | Not equal             | `!=3.8`                          |
| `<`      | Less than             | `<3.12`                          |
| `<=`     | Less than or equal    | `<=3.11`                         |
| `>`      | Greater than          | `>3.9`                           |
| `>=`     | Greater than or equal | `>=3.9`                          |
| `~=`     | Compatible release    | `~=3.11.0` (>= 3.11.0, < 3.12.0) |
| `*`      | Wildcard              | `3.11.*` (any 3.11.x)            |
| `,`      | AND                   | `">=3.9,<3.12"`                  |
| \`       | \`                    | OR                               |

### Build Strings

Build strings identify specific builds of the same package version. They're especially important for packages that have different builds for:

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

The build number is an integer that increments each time a package is rebuilt with the same version. Use build number constraints when you need a specific rebuild of a package:

```shell
# Specific build number
pixi add "python [version='3.11.0', build_number='1']"

# Build number constraint
pixi add "numpy [build_number='>=5']"
```

pixi.toml

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

Channels are repositories where conda packages are hosted. You can specify which channel to fetch a package from:

```shell
# Specific channel by name
pixi add "pytorch [channel='pytorch']"

# Channel URL
pixi add "custom-package [channel='https://prefix.dev/my-channel']"

# Or use the shorter `::` syntax
pixi add pytorch::pytorch
pixi add https://prefix.dev/my-channel::custom-package
```

pixi.toml

```toml
[dependencies.pytorch]
channel = "pytorch"

[dependencies.custom-package]
channel = "https://prefix.dev/my-channel"
```

Note that for `pixi add`, channels must also be listed in your workspace configuration:

pixi.toml

```toml
[workspace]
channels = ["conda-forge", "pytorch", "nvidia"]
```

You can also add these using the command line:

```shell
pixi workspace channel add conda-forge
```

### Checksums (SHA256/MD5)

Checksums verify package integrity and authenticity. Use them for reproducibility and security:

pixi.toml

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

Specify the expected license of a package. This is useful for:

- Ensuring compliance with your organization's policies
- Filtering packages by license type
- Documentation purposes

pixi.toml

```toml
[dependencies.pytorch]
version = "2.0.*"
license = "BSD-3-Clause"
```

### File Name

Specify the exact package file name to download. This is rarely needed but useful for:

- Debugging package resolution issues
- Ensuring a specific package artifact
- Advanced use cases with custom package builds

pixi.toml

```toml
[dependencies.pytorch]
file-name = "pytorch-2.0.0-cuda.tar.bz2"
```

## Source Packages

Warning

`pixi-build` is a preview feature, and will change until it is stabilized. Please keep that in mind when you use it for your workspaces.

For these packages to be recognized they need to be understood by Pixi as source packages. Look at the [Pixi Manifest Reference](../../reference/pixi_manifest/#the-package-section) for more information on how to declare source packages in your `pixi.toml`.

### Path-based Source Packages

pixi.toml

```toml
[dependencies.local-package]
# Local file path to a pixi package
path = "/path/to/package"
```

The path should be relative to the workspace root or an absolute path, but absolute paths are discouraged for portability.

### Git-based Source Packages

pixi.toml

```toml
# Git repository of a Pixi package
[dependencies.git-package]
# Git repository
git = "https://github.com/org/repo"
# Git branch
branch = "main"
# Subdirectory within repo
subdirectory = "packages/mypackage"

[dependencies.tagged-git-package]
# Git with specific tag
git = "https://github.com/org/repo"
tag = "v1.0.0"

[dependencies.rev-git-package]
# Git with specific revision
git = "https://github.com/org/repo"
rev = "abc123def"
```

For git repositories, you can specify:

- **git**: Repository URL
- **branch**: Git branch name
- **tag**: Git tag
- **rev**: Specific git revision/commit SHA
- **subdirectory**: Path within the repository

## PyPI package specifications

Pixi also supports installing package dependencies from PyPI using `pixi add --pypi` and in your `pixi.toml` and `pyproject.toml`. Pixi implements the standard [PEP440 version specifiers](https://peps.python.org/pep-0440/) for specifying package versions.

### Command Line Syntax

When using `pixi add --pypi`, you can [specify packages similarly to `pip`](https://pip.pypa.io/en/stable/user_guide/#installing-packages):

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

pixi.toml

```toml
[pypi-dependencies]
# Version specification
black = "==22.3.0"
ruff = ">=0.0.241,<1.0.0"

# Specific index URL
pytest = { version = "==7.2.0", index = "https://pypi.org/simple" }

# Extras
fastapi = { version = "==0.78.0", extras = ["all"] }

# URL
uvicorn = { url = "https://files.pythonhosted.org/packages/1e/db/4254e3eabe8020b458f1a747140d32277ec7a271daf1d235b70dc0b4e6e3/requests-2.32.5-py3-none-any.whl#sha256=2462f94637a34fd532264295e186976db0f5d453d1cdd31473c85a6a161affb6" }

# Git repository
requests0 = { git = "https://github.com/psf/requests.git", rev = "70298332899f25826e35e42f8d83425124f755a" }
requests1 = { git = "https://github.com/psf/requests.git", branch = "main" }
requests2 = { git = "https://github.com/psf/requests.git", tag = "v2.28.1" }
requests3 = { git = "https://github.com/psf/requests.git", subdirectory = "requests" }

# Local path
local_package = { path = "../local_package" }
local_package2 = { path = "../local_package2", extras = ["extra_feature"] }
local_package3 = { path = "../local_package3", editable = true }
```

## Further Reading

- [Pixi Manifest Reference](../../reference/pixi_manifest/#dependencies) - Complete dependency specification options
- [Multi-Platform Configuration](../../workspace/multi_platform_configuration/) - Platform-specific dependencies
- [Conda Package Specification](https://conda.io/projects/conda/en/latest/user-guide/concepts/pkg-specs.html) - Conda's package specification docs
