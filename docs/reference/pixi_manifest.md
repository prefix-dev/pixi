# `pixi.toml` manifest file

*The configuration for a [`pixi`](https://pixi.sh) project.*

## Properties

- **`$schema`** *(string, format: uri-reference)*: The schema identifier for the project's configuration. Default: `"https://pixi.sh/v0.46.0/schema/manifest/schema.json"`.
- **`activation`**: The scripts used on the activation of the project. Refer to *[#/$defs/Activation](#%24defs/Activation)*.
- **`build-dependencies`** *(object)*: The build `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
  - **Additional properties**
    - **Any of**
      - *string*: Length must be at least 1.
      - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
- **`dependencies`** *(object)*: The `conda` dependencies, consisting of a package name and a requirement in [MatchSpec](https://github.com/conda/conda/blob/078e7ee79381060217e1ec7f9b0e9cf80ecc8f3f/conda/models/match_spec.py) format. Can contain additional properties.
  - **Additional properties**
    - **Any of**
      - *string*: Length must be at least 1.
      - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
- **`environments`** *(object)*: The environments of the project, defined as a full object or a list of feature names.
  - **`^[a-z\d\-]+$`**
    - **Any of**
      - : Refer to *[#/$defs/Environment](#%24defs/Environment)*.
      - *array*
        - **Items** *(string)*: Length must be at least 1.
- **`feature`** *(object)*: The features of the project. Can contain additional properties.
  - **Additional properties**: Refer to *[#/$defs/Feature](#%24defs/Feature)*.
- **`host-dependencies`** *(object)*: The host `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
  - **Additional properties**
    - **Any of**
      - *string*: Length must be at least 1.
      - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.

  Examples:
  ```json
  {
      "python": ">=3.8"
  }
  ```

- **`package`**: The package's metadata information. Refer to *[#/$defs/Package](#%24defs/Package)*.
- **`project`**: The project's metadata information. Refer to *[#/$defs/Workspace](#%24defs/Workspace)*.
- **`pypi-dependencies`** *(object)*: The PyPI dependencies. Can contain additional properties.
  - **Additional properties**
    - **Any of**
      - *string*: Length must be at least 1.
      - : Refer to *[#/$defs/PyPIVersion](#%24defs/PyPIVersion)*.
      - : Refer to *[#/$defs/PyPIGitBranchRequirement](#%24defs/PyPIGitBranchRequirement)*.
      - : Refer to *[#/$defs/PyPIGitTagRequirement](#%24defs/PyPIGitTagRequirement)*.
      - : Refer to *[#/$defs/PyPIGitRevRequirement](#%24defs/PyPIGitRevRequirement)*.
      - : Refer to *[#/$defs/PyPIPathRequirement](#%24defs/PyPIPathRequirement)*.
      - : Refer to *[#/$defs/PyPIUrlRequirement](#%24defs/PyPIUrlRequirement)*.
- **`pypi-options`**: Options related to PyPI indexes, on the default feature. Refer to *[#/$defs/PyPIOptions](#%24defs/PyPIOptions)*.
- **`system-requirements`**: The system requirements of the project. Refer to *[#/$defs/SystemRequirements](#%24defs/SystemRequirements)*.
- **`target`** *(object)*: The targets of the project. Can contain additional properties.
  - **Additional properties**: Refer to *[#/$defs/Target](#%24defs/Target)*.

  Examples:
  ```json
  {
      "linux": {
          "dependencies": {
              "python": "3.8"
          }
      }
  }
  ```

- **`tasks`** *(object)*: The tasks of the project.
  - **`^[^\s\$]+$`**
    - **Any of**
      - : Refer to *[#/$defs/TaskInlineTable](#%24defs/TaskInlineTable)*.
      - *array*
        - **Items**: Refer to *[#/$defs/DependsOn](#%24defs/DependsOn)*.
      - *string*: Length must be at least 1.
- **`tool`** *(object)*: Third-party tool configurations, ignored by pixi. Can contain additional properties.
- **`workspace`**: The workspace's metadata information. Refer to *[#/$defs/Workspace](#%24defs/Workspace)*.
## Definitions

- <a id="%24defs/Activation"></a>**`Activation`** *(object)*: A description of steps performed when an environment is activated. Cannot contain additional properties.
  - **`env`** *(object)*: A map of environment variables to values, used in the activation of the environment. These will be set in the shell. Thus these variables are shell specific. Using '$' might not expand to a value in different shells. Can contain additional properties.
    - **Additional properties** *(string)*: Length must be at least 1.

    Examples:
    ```json
    {
        "key": "value"
    }
    ```

    ```json
    {
        "ARGUMENT": "value"
    }
    ```

  - **`scripts`** *(array)*: The scripts to run when the environment is activated.
    - **Items** *(string)*: Length must be at least 1.

    Examples:
    ```json
    "activate.sh"
    ```

    ```json
    "activate.bat"
    ```

- <a id="%24defs/Build"></a>**`Build`** *(object)*: Cannot contain additional properties.
  - **`additional-dependencies`** *(object)*: Additional dependencies to install alongside the build backend. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
  - **`backend`**: The build backend to instantiate. Refer to *[#/$defs/BuildBackend](#%24defs/BuildBackend)*.
  - **`channels`** *(array)*: The `conda` channels that are used to fetch the build backend from.
    - **Items**
      - **Any of**
        - *string*: Length must be at least 1.
        - *string, format: uri*: Length must be at least 1.
        - : Refer to *[#/$defs/ChannelInlineTable](#%24defs/ChannelInlineTable)*.
  - **`configuration`** *(object)*: The configuration of the build backend. Can contain additional properties.
- <a id="%24defs/BuildBackend"></a>**`BuildBackend`** *(object)*: Cannot contain additional properties.
  - **`branch`** *(string)*: A git branch to use. Length must be at least 1.
  - **`build`** *(string)*: The build string of the package. Length must be at least 1.
  - **`build-number`** *(string)*: The build number of the package, can be a spec like `>=1` or `<=10` or `1`. Length must be at least 1.
  - **`channel`** *(string)*: The channel the packages needs to be fetched from. Length must be at least 1.

    Examples:
    ```json
    "conda-forge"
    ```

    ```json
    "pytorch"
    ```

    ```json
    "https://repo.prefix.dev/conda-forge"
    ```

  - **`file-name`** *(string)*: The file name of the package. Length must be at least 1.
  - **`git`** *(string)*: The git URL to the repo. Length must be at least 1.
  - **`license`** *(string)*: The license of the package. Length must be at least 1.
  - **`md5`** *(string)*: The md5 hash of the package. Must match pattern: `^[a-fA-F0-9]{32}$` ([Test](https://regexr.com/?expression=%5E%5Ba-fA-F0-9%5D%7B32%7D%24)).
  - **`name`** *(string)*: The name of the build backend package. Length must be at least 1.
  - **`path`** *(string)*: The path to the package. Length must be at least 1.
  - **`rev`** *(string)*: A git SHA revision to use. Length must be at least 1.
  - **`sha256`** *(string)*: The sha256 hash of the package. Must match pattern: `^[a-fA-F0-9]{64}$` ([Test](https://regexr.com/?expression=%5E%5Ba-fA-F0-9%5D%7B64%7D%24)).
  - **`subdir`** *(string)*: The subdir of the package, also known as platform. Length must be at least 1.
  - **`subdirectory`** *(string)*: A subdirectory to use in the repo. Length must be at least 1.
  - **`tag`** *(string)*: A git tag to use. Length must be at least 1.
  - **`url`** *(string)*: The URL to the package. Length must be at least 1.
  - **`version`** *(string)*: The version of the package in [MatchSpec](https://github.com/conda/conda/blob/078e7ee79381060217e1ec7f9b0e9cf80ecc8f3f/conda/models/match_spec.py) format. Length must be at least 1.
- <a id="%24defs/ChannelInlineTable"></a>**`ChannelInlineTable`** *(object)*: A precise description of a `conda` channel, with an optional priority. Cannot contain additional properties.
  - **`channel`**: The channel the packages needs to be fetched from.
    - **Any of**
      - *string*: Length must be at least 1.
      - *string, format: uri*: Length must be at least 1.
  - **`priority`** *(integer)*: The priority of the channel.
- <a id="%24defs/ChannelPriority"></a>**`ChannelPriority`** *(string)*: The priority of the channel. Must be one of: `["disabled", "strict"]`.
- <a id="%24defs/DependsOn"></a>**`DependsOn`** *(object)*: The dependencies of a task. Cannot contain additional properties.
  - **`args`** *(array)*: The arguments to pass to the task.
    - **Items** *(string)*: Length must be at least 1.
  - **`environment`** *(string)*: The environment to use for the task. Must match pattern: `^[a-z\d\-]+$` ([Test](https://regexr.com/?expression=%5E%5Ba-z%5Cd%5C-%5D%2B%24)).
  - **`task`** *(string, required)*: A valid task name. Must match pattern: `^[^\s\$]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5Cs%5C%24%5D%2B%24)).
- <a id="%24defs/Environment"></a>**`Environment`** *(object)*: A composition of the dependencies of features which can be activated to run tasks or provide a shell. Cannot contain additional properties.
  - **`features`** *(array)*: The features that define the environment.
    - **Items** *(string)*: Length must be at least 1.
  - **`no-default-feature`** *(boolean)*: Whether to add the default feature to this environment. Default: `false`.
  - **`solve-group`** *(string)*: The group name for environments that should be solved together. Length must be at least 1.
- <a id="%24defs/Feature"></a>**`Feature`** *(object)*: A composable aspect of the project which can contribute dependencies and tasks to an environment. Cannot contain additional properties.
  - **`activation`**: The scripts used on the activation of environments using this feature. Refer to *[#/$defs/Activation](#%24defs/Activation)*.
  - **`build-dependencies`** *(object)*: The build `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
  - **`channel-priority`**: The type of channel priority that is used in the solve.- 'strict': only take the package from the channel it exist in first.- 'disabled': group all dependencies together as if there is no channel difference. Refer to *[#/$defs/ChannelPriority](#%24defs/ChannelPriority)*.

    Examples:
    ```json
    "strict"
    ```

    ```json
    "disabled"
    ```

  - **`channels`** *(array)*: The `conda` channels that can be considered when solving environments containing this feature.
    - **Items**
      - **Any of**
        - *string*: Length must be at least 1.
        - *string, format: uri*: Length must be at least 1.
        - : Refer to *[#/$defs/ChannelInlineTable](#%24defs/ChannelInlineTable)*.
  - **`dependencies`** *(object)*: The `conda` dependencies, consisting of a package name and a requirement in [MatchSpec](https://github.com/conda/conda/blob/078e7ee79381060217e1ec7f9b0e9cf80ecc8f3f/conda/models/match_spec.py) format. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
  - **`host-dependencies`** *(object)*: The host `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.

    Examples:
    ```json
    {
        "python": ">=3.8"
    }
    ```

  - **`platforms`** *(array)*: The platforms that the feature supports: a union of all features combined in one environment is used for the environment.
    - **Items**: Refer to *[#/$defs/Platform](#%24defs/Platform)*.
  - **`pypi-dependencies`** *(object)*: The PyPI dependencies of this feature. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/PyPIVersion](#%24defs/PyPIVersion)*.
        - : Refer to *[#/$defs/PyPIGitBranchRequirement](#%24defs/PyPIGitBranchRequirement)*.
        - : Refer to *[#/$defs/PyPIGitTagRequirement](#%24defs/PyPIGitTagRequirement)*.
        - : Refer to *[#/$defs/PyPIGitRevRequirement](#%24defs/PyPIGitRevRequirement)*.
        - : Refer to *[#/$defs/PyPIPathRequirement](#%24defs/PyPIPathRequirement)*.
        - : Refer to *[#/$defs/PyPIUrlRequirement](#%24defs/PyPIUrlRequirement)*.
  - **`pypi-options`**: Options related to PyPI indexes for this feature. Refer to *[#/$defs/PyPIOptions](#%24defs/PyPIOptions)*.
  - **`system-requirements`**: The system requirements of this feature. Refer to *[#/$defs/SystemRequirements](#%24defs/SystemRequirements)*.
  - **`target`** *(object)*: Machine-specific aspects of this feature. Can contain additional properties.
    - **Additional properties**: Refer to *[#/$defs/Target](#%24defs/Target)*.

    Examples:
    ```json
    {
        "linux": {
            "dependencies": {
                "python": "3.8"
            }
        }
    }
    ```

  - **`tasks`** *(object)*: The tasks provided by this feature.
    - **`^[^\s\$]+$`**
      - **Any of**
        - : Refer to *[#/$defs/TaskInlineTable](#%24defs/TaskInlineTable)*.
        - *array*
          - **Items**: Refer to *[#/$defs/DependsOn](#%24defs/DependsOn)*.
        - *string*: Length must be at least 1.
- <a id="%24defs/FindLinksPath"></a>**`FindLinksPath`** *(object)*: The path to the directory containing packages. Cannot contain additional properties.
  - **`path`** *(string)*: Path to the directory of packages. Length must be at least 1.

    Examples:
    ```json
    "./links"
    ```

- <a id="%24defs/FindLinksURL"></a>**`FindLinksURL`** *(object)*: The URL to the html file containing href-links to packages. Cannot contain additional properties.
  - **`url`** *(string)*: URL to html file with href-links to packages. Length must be at least 1.

    Examples:
    ```json
    "https://simple-index-is-here.com"
    ```

- <a id="%24defs/LibcFamily"></a>**`LibcFamily`** *(object)*: Cannot contain additional properties.
  - **`family`** *(string)*: The family of the `libc`. Length must be at least 1.

    Examples:
    ```json
    "glibc"
    ```

    ```json
    "musl"
    ```

  - **`version`**: The version of `libc`.
    - **Any of**
      - *number*
      - *string*: Length must be at least 1.
- <a id="%24defs/MatchspecTable"></a>**`MatchspecTable`** *(object)*: A precise description of a `conda` package version. Cannot contain additional properties.
  - **`branch`** *(string)*: A git branch to use. Length must be at least 1.
  - **`build`** *(string)*: The build string of the package. Length must be at least 1.
  - **`build-number`** *(string)*: The build number of the package, can be a spec like `>=1` or `<=10` or `1`. Length must be at least 1.
  - **`channel`** *(string)*: The channel the packages needs to be fetched from. Length must be at least 1.

    Examples:
    ```json
    "conda-forge"
    ```

    ```json
    "pytorch"
    ```

    ```json
    "https://repo.prefix.dev/conda-forge"
    ```

  - **`file-name`** *(string)*: The file name of the package. Length must be at least 1.
  - **`git`** *(string)*: The git URL to the repo. Length must be at least 1.
  - **`license`** *(string)*: The license of the package. Length must be at least 1.
  - **`md5`** *(string)*: The md5 hash of the package. Must match pattern: `^[a-fA-F0-9]{32}$` ([Test](https://regexr.com/?expression=%5E%5Ba-fA-F0-9%5D%7B32%7D%24)).
  - **`path`** *(string)*: The path to the package. Length must be at least 1.
  - **`rev`** *(string)*: A git SHA revision to use. Length must be at least 1.
  - **`sha256`** *(string)*: The sha256 hash of the package. Must match pattern: `^[a-fA-F0-9]{64}$` ([Test](https://regexr.com/?expression=%5E%5Ba-fA-F0-9%5D%7B64%7D%24)).
  - **`subdir`** *(string)*: The subdir of the package, also known as platform. Length must be at least 1.
  - **`subdirectory`** *(string)*: A subdirectory to use in the repo. Length must be at least 1.
  - **`tag`** *(string)*: A git tag to use. Length must be at least 1.
  - **`url`** *(string)*: The URL to the package. Length must be at least 1.
  - **`version`** *(string)*: The version of the package in [MatchSpec](https://github.com/conda/conda/blob/078e7ee79381060217e1ec7f9b0e9cf80ecc8f3f/conda/models/match_spec.py) format. Length must be at least 1.
- <a id="%24defs/Package"></a>**`Package`** *(object)*: The package's metadata information. Cannot contain additional properties.
  - **`authors`** *(array)*: The authors of the project.
    - **Items** *(string)*: Length must be at least 1.

    Examples:
    ```json
    "John Doe <j.doe@prefix.dev>"
    ```

  - **`build`**: The build configuration of the package. Refer to *[#/$defs/Build](#%24defs/Build)*.
  - **`build-dependencies`** *(object)*: The build `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
  - **`description`** *(string)*: A short description of the project. Length must be at least 1.
  - **`documentation`** *(string, format: uri)*: The URL of the documentation of the project. Length must be at least 1.
  - **`homepage`** *(string, format: uri)*: The URL of the homepage of the project. Length must be at least 1.
  - **`host-dependencies`** *(object)*: The host `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.

    Examples:
    ```json
    {
        "python": ">=3.8"
    }
    ```

  - **`license`** *(string)*: The license of the project; we advise using an [SPDX](https://spdx.org/licenses/) identifier. Length must be at least 1.
  - **`license-file`** *(string)*: The path to the license file of the project. Must match pattern: `^[^\\]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5C%5C%5D%2B%24)).
  - **`name`** *(string)*: The name of the package. Length must be at least 1.
  - **`readme`** *(string)*: The path to the readme file of the project. Must match pattern: `^[^\\]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5C%5C%5D%2B%24)).
  - **`repository`** *(string, format: uri)*: The URL of the repository of the project. Length must be at least 1.
  - **`run-dependencies`** *(object)*: The `conda` dependencies required at runtime. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
  - **`target`** *(object)*: Machine-specific aspects of the package. Can contain additional properties.
    - **Additional properties**: Refer to *[#/$defs/Target](#%24defs/Target)*.

    Examples:
    ```json
    {
        "linux": {
            "host-dependencies": {
                "python": "3.8"
            }
        }
    }
    ```

  - **`version`** *(string)*: The version of the project; we advise use of [SemVer](https://semver.org). Length must be at least 1.

    Examples:
    ```json
    "1.2.3"
    ```

- <a id="%24defs/Platform"></a>**`Platform`** *(string)*: A supported operating system and processor architecture pair. Must be one of: `["emscripten-wasm32", "linux-32", "linux-64", "linux-aarch64", "linux-armv6l", "linux-armv7l", "linux-ppc64", "linux-ppc64le", "linux-riscv32", "linux-riscv64", "linux-s390x", "noarch", "osx-64", "osx-arm64", "unknown", "wasi-wasm32", "win-32", "win-64", "win-arm64", "zos-z"]`.
- <a id="%24defs/PyPIGitBranchRequirement"></a>**`PyPIGitBranchRequirement`** *(object)*: Cannot contain additional properties.
  - **`branch`** *(string)*: A `git` branch to use. Length must be at least 1.
  - **`extras`** *(array)*: The [PEP 508 extras](https://peps.python.org/pep-0508/#extras) of the package.
    - **Items** *(string)*: Length must be at least 1.
  - **`git`** *(string)*: The `git` URL to the repo e.g https://github.com/prefix-dev/pixi. Length must be at least 1.
  - **`subdirectory`** *(string)*: The subdirectory in the repo, a path from the root of the repo. Length must be at least 1.
- <a id="%24defs/PyPIGitRevRequirement"></a>**`PyPIGitRevRequirement`** *(object)*: Cannot contain additional properties.
  - **`extras`** *(array)*: The [PEP 508 extras](https://peps.python.org/pep-0508/#extras) of the package.
    - **Items** *(string)*: Length must be at least 1.
  - **`git`** *(string)*: The `git` URL to the repo e.g https://github.com/prefix-dev/pixi. Length must be at least 1.
  - **`rev`** *(string)*: A `git` SHA revision to use. Length must be at least 1.
  - **`subdirectory`** *(string)*: The subdirectory in the repo, a path from the root of the repo. Length must be at least 1.
- <a id="%24defs/PyPIGitTagRequirement"></a>**`PyPIGitTagRequirement`** *(object)*: Cannot contain additional properties.
  - **`extras`** *(array)*: The [PEP 508 extras](https://peps.python.org/pep-0508/#extras) of the package.
    - **Items** *(string)*: Length must be at least 1.
  - **`git`** *(string)*: The `git` URL to the repo e.g https://github.com/prefix-dev/pixi. Length must be at least 1.
  - **`subdirectory`** *(string)*: The subdirectory in the repo, a path from the root of the repo. Length must be at least 1.
  - **`tag`** *(string)*: A `git` tag to use. Length must be at least 1.
- <a id="%24defs/PyPIOptions"></a>**`PyPIOptions`** *(object)*: Options that determine the behavior of PyPI package resolution and installation. Cannot contain additional properties.
  - **`extra-index-urls`** *(array)*: Additional PyPI registries that should be used as extra indexes.
    - **Items** *(string)*: Length must be at least 1.

    Examples:
    ```json
    [
        "https://pypi.org/simple"
    ]
    ```

  - **`find-links`** *(array)*: Paths to directory containing.
    - **Items**
      - **Any of**
        - : Refer to *[#/$defs/FindLinksPath](#%24defs/FindLinksPath)*.
        - : Refer to *[#/$defs/FindLinksURL](#%24defs/FindLinksURL)*.

    Examples:
    ```json
    [
        "https://pypi.org/simple"
    ]
    ```

  - **`index-strategy`**: The strategy to use when resolving packages from multiple indexes.
    - **Any of**
      - *string*: Must be: `"first-index"`.
      - *string*: Must be: `"unsafe-first-match"`.
      - *string*: Must be: `"unsafe-best-match"`.

    Examples:
    ```json
    "first-index"
    ```

    ```json
    "unsafe-first-match"
    ```

    ```json
    "unsafe-best-match"
    ```

  - **`index-url`** *(string)*: PyPI registry that should be used as the primary index. Length must be at least 1.

    Examples:
    ```json
    "https://pypi.org/simple"
    ```

  - **`no-build`**: Packages that should NOT be built.
    - **Any of**
      - *boolean*
      - *array*
        - **Items** *(string)*: Length must be at least 1.

    Examples:
    ```json
    "true"
    ```

    ```json
    "false"
    ```

  - **`no-build-isolation`**: Packages that should NOT be isolated during the build process.
    - **Any of**
      - *boolean*
      - *array*
        - **Items** *(string)*: Length must be at least 1.

    Examples:
    ```json
    [
        "numpy"
    ]
    ```

    ```json
    true
    ```

- <a id="%24defs/PyPIPathRequirement"></a>**`PyPIPathRequirement`** *(object)*: Cannot contain additional properties.
  - **`editable`** *(boolean)*: If `true` the package will be installed as editable.
  - **`extras`** *(array)*: The [PEP 508 extras](https://peps.python.org/pep-0508/#extras) of the package.
    - **Items** *(string)*: Length must be at least 1.
  - **`path`** *(string)*: A path to a local source or wheel. Length must be at least 1.
  - **`subdirectory`** *(string)*: The subdirectory in the repo, a path from the root of the repo. Length must be at least 1.
- <a id="%24defs/PyPIUrlRequirement"></a>**`PyPIUrlRequirement`** *(object)*: Cannot contain additional properties.
  - **`extras`** *(array)*: The [PEP 508 extras](https://peps.python.org/pep-0508/#extras) of the package.
    - **Items** *(string)*: Length must be at least 1.
  - **`url`** *(string)*: A URL to a remote source or wheel. Length must be at least 1.
- <a id="%24defs/PyPIVersion"></a>**`PyPIVersion`** *(object)*: Cannot contain additional properties.
  - **`extras`** *(array)*: The [PEP 508 extras](https://peps.python.org/pep-0508/#extras) of the package.
    - **Items** *(string)*: Length must be at least 1.
  - **`index`** *(string)*: The index to fetch the package from. Length must be at least 1.
  - **`version`** *(string)*: The version of the package in [PEP 440](https://www.python.org/dev/peps/pep-0440/) format. Length must be at least 1.
- <a id="%24defs/S3Options"></a>**`S3Options`** *(object)*: Options related to S3 for this project. Cannot contain additional properties.
  - **`endpoint-url`** *(string, required)*: The endpoint URL to use for the S3 client. Length must be at least 1.

    Examples:
    ```json
    "https://s3.eu-central-1.amazonaws.com"
    ```

  - **`force-path-style`** *(boolean, required)*: Whether to force path style for the S3 client.
  - **`region`** *(string, required)*: The region to use for the S3 client. Length must be at least 1.

    Examples:
    ```json
    "eu-central-1"
    ```

- <a id="%24defs/SystemRequirements"></a>**`SystemRequirements`** *(object)*: Platform-specific requirements. Cannot contain additional properties.
  - **`archspec`** *(string)*: The architecture the project supports. Length must be at least 1.
  - **`cuda`**: The minimum version of CUDA.
    - **Any of**
      - *number*
      - *string*: Length must be at least 1.
  - **`libc`**: The minimum version of `libc`.
    - **Any of**
      - : Refer to *[#/$defs/LibcFamily](#%24defs/LibcFamily)*.
      - *number*
      - *string*: Length must be at least 1.
  - **`linux`**: The minimum version of the Linux kernel.
    - **Any of**
      - *number*: Exclusive minimum: `0`.
      - *string*: Length must be at least 1.
  - **`macos`**: The minimum version of MacOS.
    - **Any of**
      - *number*: Exclusive minimum: `0`.
      - *string*: Length must be at least 1.
  - **`unix`**: Whether the project supports UNIX.
    - **Any of**
      - *boolean*
      - *string*: Length must be at least 1.

    Examples:
    ```json
    "true"
    ```

- <a id="%24defs/Target"></a>**`Target`** *(object)*: A machine-specific configuration of dependencies and tasks. Cannot contain additional properties.
  - **`activation`**: The scripts used on the activation of the project for this target. Refer to *[#/$defs/Activation](#%24defs/Activation)*.
  - **`build-dependencies`** *(object)*: The build `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
  - **`dependencies`** *(object)*: The `conda` dependencies, consisting of a package name and a requirement in [MatchSpec](https://github.com/conda/conda/blob/078e7ee79381060217e1ec7f9b0e9cf80ecc8f3f/conda/models/match_spec.py) format. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.
  - **`host-dependencies`** *(object)*: The host `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/MatchspecTable](#%24defs/MatchspecTable)*.

    Examples:
    ```json
    {
        "python": ">=3.8"
    }
    ```

  - **`pypi-dependencies`** *(object)*: The PyPI dependencies for this target. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string*: Length must be at least 1.
        - : Refer to *[#/$defs/PyPIVersion](#%24defs/PyPIVersion)*.
        - : Refer to *[#/$defs/PyPIGitBranchRequirement](#%24defs/PyPIGitBranchRequirement)*.
        - : Refer to *[#/$defs/PyPIGitTagRequirement](#%24defs/PyPIGitTagRequirement)*.
        - : Refer to *[#/$defs/PyPIGitRevRequirement](#%24defs/PyPIGitRevRequirement)*.
        - : Refer to *[#/$defs/PyPIPathRequirement](#%24defs/PyPIPathRequirement)*.
        - : Refer to *[#/$defs/PyPIUrlRequirement](#%24defs/PyPIUrlRequirement)*.
  - **`tasks`** *(object)*: The tasks of the target.
    - **`^[^\s\$]+$`**
      - **Any of**
        - : Refer to *[#/$defs/TaskInlineTable](#%24defs/TaskInlineTable)*.
        - *array*
          - **Items**: Refer to *[#/$defs/DependsOn](#%24defs/DependsOn)*.
        - *string*: Length must be at least 1.
- <a id="%24defs/TaskArgs"></a>**`TaskArgs`** *(object)*: The arguments of a task. Cannot contain additional properties.
  - **`arg`** *(string, required)*: Length must be at least 1.
  - **`default`** *(string)*: The default value of the argument. Length must be at least 1.
- <a id="%24defs/TaskInlineTable"></a>**`TaskInlineTable`** *(object)*: A precise definition of a task. Cannot contain additional properties.
  - **`args`** *(array)*: The arguments to pass to the task.
    - **Items**
      - **Any of**
        - : Refer to *[#/$defs/TaskArgs](#%24defs/TaskArgs)*.
        - *string*: Length must be at least 1.

    Examples:
    ```json
    "arg1"
    ```

    ```json
    "arg2"
    ```

  - **`clean-env`** *(boolean)*: Whether to run in a clean environment, removing all environment variables except those defined in `env` and by pixi itself.
  - **`cmd`**: A shell command to run the task in the limited, but cross-platform `bash`-like `deno_task_shell`. See the documentation for [supported syntax](https://pixi.sh/latest/environments/advanced_tasks/#syntax).
    - **Any of**
      - *array*
        - **Items** *(string)*: Length must be at least 1.
      - *string*: Length must be at least 1.
  - **`cwd`** *(string)*: The working directory to run the task. Must match pattern: `^[^\\]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5C%5C%5D%2B%24)).
  - **`depends-on`**: The tasks that this task depends on. Environment variables will **not** be expanded.
    - **Any of**
      - *array*
        - **Items**
          - **Any of**
            - : Refer to *[#/$defs/DependsOn](#%24defs/DependsOn)*.
            - *string*: A valid task name. Must match pattern: `^[^\s\$]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5Cs%5C%24%5D%2B%24)).
      - : Refer to *[#/$defs/DependsOn](#%24defs/DependsOn)*.
      - *string*: A valid task name. Must match pattern: `^[^\s\$]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5Cs%5C%24%5D%2B%24)).
  - **`depends_on`**: The tasks that this task depends on. Environment variables will **not** be expanded. Deprecated in favor of `depends-on` from v0.21.0 onward.
    - **Any of**
      - *array*
        - **Items** *(string)*: A valid task name. Must match pattern: `^[^\s\$]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5Cs%5C%24%5D%2B%24)).
      - *string*: A valid task name. Must match pattern: `^[^\s\$]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5Cs%5C%24%5D%2B%24)).
  - **`description`** *(string)*: A short description of the task. Length must be at least 1.

    Examples:
    ```json
    "Build the project"
    ```

  - **`env`** *(object)*: A map of environment variables to values, used in the task, these will be overwritten by the shell. Can contain additional properties.
    - **Additional properties** *(string)*: Length must be at least 1.

    Examples:
    ```json
    {
        "key": "value"
    }
    ```

    ```json
    {
        "ARGUMENT": "value"
    }
    ```

  - **`inputs`** *(array)*: A list of `.gitignore`-style glob patterns that should be watched for changes before this command is run. Environment variables _will_ be expanded.
    - **Items** *(string)*: Length must be at least 1.
  - **`outputs`** *(array)*: A list of `.gitignore`-style glob patterns that are generated by this command. Environment variables _will_ be expanded.
    - **Items** *(string)*: Length must be at least 1.
- <a id="%24defs/Workspace"></a>**`Workspace`** *(object)*: The project's metadata information. Cannot contain additional properties.
  - **`authors`** *(array)*: The authors of the project.
    - **Items** *(string)*: Length must be at least 1.

    Examples:
    ```json
    "John Doe <j.doe@prefix.dev>"
    ```

  - **`build-variants`** *(object)*: The build variants of the project. Can contain additional properties.
    - **Additional properties** *(array)*
      - **Items** *(string)*
  - **`channel-priority`**: The type of channel priority that is used in the solve.- 'strict': only take the package from the channel it exist in first.- 'disabled': group all dependencies together as if there is no channel difference. Refer to *[#/$defs/ChannelPriority](#%24defs/ChannelPriority)*.

    Examples:
    ```json
    "strict"
    ```

    ```json
    "disabled"
    ```

  - **`channels`** *(array, required)*: The `conda` channels that can be used in the project. Unless overridden by `priority`, the first channel listed will be preferred.
    - **Items**
      - **Any of**
        - *string*: Length must be at least 1.
        - *string, format: uri*: Length must be at least 1.
        - : Refer to *[#/$defs/ChannelInlineTable](#%24defs/ChannelInlineTable)*.
  - **`conda-pypi-map`** *(object)*: The `conda` to PyPI mapping configuration. Can contain additional properties.
    - **Additional properties**
      - **Any of**
        - *string, format: uri*: Length must be at least 1.
        - *string*: Length must be at least 1.
  - **`description`** *(string)*: A short description of the project. Length must be at least 1.
  - **`documentation`** *(string, format: uri)*: The URL of the documentation of the project. Length must be at least 1.
  - **`exclude-newer`** *(string)*: Exclude any package newer than this date. Must match pattern: `^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}(Z|[+-]\d{2}:\d{2}))?$` ([Test](https://regexr.com/?expression=%5E%5Cd%7B4%7D-%5Cd%7B2%7D-%5Cd%7B2%7D%28T%5Cd%7B2%7D%3A%5Cd%7B2%7D%3A%5Cd%7B2%7D%28Z%7C%5B%2B-%5D%5Cd%7B2%7D%3A%5Cd%7B2%7D%29%29%3F%24)).

    Examples:
    ```json
    "2023-11-03"
    ```

    ```json
    "2023-11-03T03:33:12Z"
    ```

  - **`homepage`** *(string, format: uri)*: The URL of the homepage of the project. Length must be at least 1.
  - **`license`** *(string)*: The license of the project; we advise using an [SPDX](https://spdx.org/licenses/) identifier. Length must be at least 1.
  - **`license-file`** *(string)*: The path to the license file of the project. Must match pattern: `^[^\\]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5C%5C%5D%2B%24)).
  - **`name`** *(string)*: The name of the project; we advise use of the name of the repository. Length must be at least 1.
  - **`platforms`** *(array)*: The platforms that the project supports.
    - **Items**: Refer to *[#/$defs/Platform](#%24defs/Platform)*.
  - **`preview`**: Defines the enabling of preview features of the project.
    - **Any of**
      - *array*
        - **Items**
          - **Any of**
            - *string*: Enables building of source records. Must be: `"pixi-build"`.
            - *string*
      - *boolean*
  - **`pypi-options`**: Options related to PyPI indexes for this project. Refer to *[#/$defs/PyPIOptions](#%24defs/PyPIOptions)*.
  - **`readme`** *(string)*: The path to the readme file of the project. Must match pattern: `^[^\\]+$` ([Test](https://regexr.com/?expression=%5E%5B%5E%5C%5C%5D%2B%24)).
  - **`repository`** *(string, format: uri)*: The URL of the repository of the project. Length must be at least 1.
  - **`requires-pixi`** *(string)*: The required version spec for pixi itself to resolve and build the project. Length must be at least 1.

    Examples:
    ```json
    ">=0.40"
    ```

  - **`s3-options`** *(object)*: Options related to S3 for this project. Can contain additional properties.
    - **Additional properties**: Refer to *[#/$defs/S3Options](#%24defs/S3Options)*.
  - **`version`** *(string)*: The version of the project; we advise use of [SemVer](https://semver.org). Length must be at least 1.

    Examples:
    ```json
    "1.2.3"
    ```

