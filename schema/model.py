"""A canonical schema definition for the ``pixi.toml`` manifest file."""

from __future__ import annotations

import json
from copy import deepcopy
from pathlib import Path
import tomllib
from typing import Annotated, Any, Optional, Literal
from enum import Enum

from pydantic import (
    AnyHttpUrl,
    BaseModel,
    Field,
    PositiveFloat,
    StringConstraints,
)

#: latest version currently supported by the `taplo` TOML linter and language server
SCHEMA_DRAFT = "http://json-schema.org/draft-07/schema#"
CARGO_TOML = Path(__file__).parent.parent / "crates" / "pixi" / "Cargo.toml"
CARGO_TOML_DATA = tomllib.loads(CARGO_TOML.read_text(encoding="utf-8"))
VERSION = CARGO_TOML_DATA["package"]["version"]
SCHEMA_URI = f"https://pixi.sh/v{VERSION}/schema/manifest/schema.json"

NonEmptyStr = Annotated[str, StringConstraints(min_length=1)]
Md5Sum = Annotated[str, StringConstraints(pattern=r"^[a-fA-F0-9]{32}$")]
Sha256Sum = Annotated[str, StringConstraints(pattern=r"^[a-fA-F0-9]{64}$")]
PathNoBackslash = Annotated[str, StringConstraints(pattern=r"^[^\\]+$")]
Glob = NonEmptyStr
UnsignedInt = Annotated[int, Field(strict=True, ge=0)]
GitUrl = Annotated[
    str, StringConstraints(pattern=r"((git|ssh|http(s)?)|(git@[\w\.]+))(:(\/\/)?)([\w\.@:\/\\-~]+)")
]
ExcludeNewer = Annotated[
    str,
    StringConstraints(pattern=r"^\d{4}-\d{2}-\d{2}([T ]\d{2}:\d{2}:\d{2}(Z|[+-]\d{2}:\d{2}))?$"),
]


def hyphenize(field: str):
    return field.replace("_", "-")


class Platform(str, Enum):
    """A supported operating system and processor architecture pair."""

    emscripten_wasm32 = "emscripten-wasm32"
    linux_32 = "linux-32"
    linux_64 = "linux-64"
    linux_aarch64 = "linux-aarch64"
    linux_armv6l = "linux-armv6l"
    linux_armv7l = "linux-armv7l"
    linux_ppc64 = "linux-ppc64"
    linux_ppc64le = "linux-ppc64le"
    linux_riscv32 = "linux-riscv32"
    linux_riscv64 = "linux-riscv64"
    linux_s390x = "linux-s390x"
    noarch = "noarch"
    osx_64 = "osx-64"
    osx_arm64 = "osx-arm64"
    unknown = "unknown"
    wasi_wasm32 = "wasi-wasm32"
    win_32 = "win-32"
    win_64 = "win-64"
    win_arm64 = "win-arm64"
    zos_z = "zos-z"


class StrictBaseModel(BaseModel):
    class Config:
        extra = "forbid"
        alias_generator = hyphenize


class WorkspaceInheritance(StrictBaseModel):
    """Indicates that a field should inherit its value from the workspace."""

    workspace: Literal[True] = Field(description="Must be true to inherit from workspace")


###################
# Project section #
###################
ChannelName = NonEmptyStr | AnyHttpUrl


class ChannelInlineTable(StrictBaseModel):
    """A precise description of a `conda` channel, with an optional priority."""

    channel: ChannelName = Field(description="The channel the packages needs to be fetched from")
    priority: int | None = Field(None, description="The priority of the channel")


Channel = ChannelName | ChannelInlineTable


class ChannelPriority(str, Enum):
    """The priority of the channel."""

    disabled = "disabled"
    strict = "strict"


PixiBuildFeature = Annotated[
    Literal["pixi-build"], Field(description="Enables building of source records")
]
KnownPreviewFeature = PixiBuildFeature


# class KnownPreviewFeature(Enum):
#     """The preview features of the project."""
#     PixiBuild: Annotated[str, Field(description="Enables building of source records")] = "pixi-build"


class Workspace(StrictBaseModel):
    """The project's metadata information."""

    name: NonEmptyStr | None = Field(
        None, description="The name of the project; we advise use of the name of the repository"
    )
    version: NonEmptyStr | None = Field(
        None,
        description="The version of the project; we advise use of [SemVer](https://semver.org)",
        examples=["1.2.3"],
    )
    description: NonEmptyStr | None = Field(None, description="A short description of the project")
    authors: list[NonEmptyStr] | None = Field(
        None, description="The authors of the project", examples=["John Doe <j.doe@prefix.dev>"]
    )
    channels: list[Channel] = Field(
        description="The `conda` channels that can be used in the project. Unless overridden by `priority`, the first channel listed will be preferred.",
    )
    channel_priority: ChannelPriority | None = Field(
        None,
        examples=["strict", "disabled"],
        description="The type of channel priority that is used in the solve."
        "- 'strict': only take the package from the channel it exist in first."
        "- 'disabled': group all dependencies together as if there is no channel difference.",
    )
    exclude_newer: ExcludeNewer | None = Field(
        None,
        examples=["2023-11-03", "2023-11-03T03:33:12Z"],
        description="Exclude any package newer than this date",
    )
    platforms: list[Platform] | None = Field(
        None, description="The platforms that the project supports"
    )
    license: NonEmptyStr | None = Field(
        None,
        description="The license of the project; we advise using an [SPDX](https://spdx.org/licenses/) identifier.",
    )
    license_file: PathNoBackslash | None = Field(
        None, description="The path to the license file of the project"
    )
    readme: PathNoBackslash | None = Field(
        None, description="The path to the readme file of the project"
    )
    homepage: AnyHttpUrl | None = Field(None, description="The URL of the homepage of the project")
    repository: AnyHttpUrl | None = Field(
        None, description="The URL of the repository of the project"
    )
    documentation: AnyHttpUrl | None = Field(
        None, description="The URL of the documentation of the project"
    )
    conda_pypi_map: dict[ChannelName, AnyHttpUrl | NonEmptyStr] | None = Field(
        None, description="The `conda` to PyPI mapping configuration"
    )
    pypi_options: PyPIOptions | None = Field(
        None, description="Options related to PyPI indexes for this project"
    )
    s3_options: dict[str, S3Options] | None = Field(
        None, description="Options related to S3 for this project"
    )
    preview: list[KnownPreviewFeature | str] | bool | None = Field(
        None, description="Defines the enabling of preview features of the project"
    )
    build_variants: dict[NonEmptyStr, list[str]] | None = Field(
        None, description="The build variants of the project"
    )
    requires_pixi: NonEmptyStr | None = Field(
        None,
        description="The required version spec for pixi itself to resolve and build the project.",
        examples=[">=0.40"],
    )
    target: dict[TargetName, WorkspaceTarget] | None = Field(
        None, description="The workspace targets"
    )


########################
# Dependencies section #
########################


class MatchspecTable(StrictBaseModel):
    """A precise description of a `conda` package version."""

    version: NonEmptyStr | None = Field(
        None,
        description="The version of the package in [MatchSpec](https://github.com/conda/conda/blob/078e7ee79381060217e1ec7f9b0e9cf80ecc8f3f/conda/models/match_spec.py) format",
    )
    build: NonEmptyStr | None = Field(None, description="The build string of the package")
    build_number: NonEmptyStr | None = Field(
        None,
        description="The build number of the package, can be a spec like `>=1` or `<=10` or `1`",
    )
    file_name: NonEmptyStr | None = Field(None, description="The file name of the package")
    channel: NonEmptyStr | None = Field(
        None,
        description="The channel the packages needs to be fetched from",
        examples=["conda-forge", "pytorch", "https://prefix.dev/conda-forge"],
    )
    subdir: NonEmptyStr | None = Field(
        None, description="The subdir of the package, also known as platform"
    )
    license: NonEmptyStr | None = Field(None, description="The license of the package")

    path: NonEmptyStr | None = Field(None, description="The path to the package")

    url: NonEmptyStr | None = Field(None, description="The URL to the package")
    md5: Md5Sum | None = Field(None, description="The md5 hash of the package")
    sha256: Sha256Sum | None = Field(None, description="The sha256 hash of the package")

    git: NonEmptyStr | None = Field(None, description="The git URL to the repo")
    rev: NonEmptyStr | None = Field(None, description="A git SHA revision to use")
    tag: NonEmptyStr | None = Field(None, description="A git tag to use")
    branch: NonEmptyStr | None = Field(None, description="A git branch to use")
    subdirectory: NonEmptyStr | None = Field(None, description="A subdirectory to use in the repo")


MatchSpec = NonEmptyStr | MatchspecTable
CondaPackageName = NonEmptyStr


class _PyPIRequirement(StrictBaseModel):
    extras: list[NonEmptyStr] | None = Field(
        None,
        description="The [PEP 508 extras](https://peps.python.org/pep-0508/#extras) of the package",
    )


class _PyPiGitRequirement(_PyPIRequirement):
    git: NonEmptyStr = Field(
        None,
        description="The `git` URL to the repo e.g https://github.com/prefix-dev/pixi",
    )
    subdirectory: NonEmptyStr | None = Field(
        None, description="The subdirectory in the repo, a path from the root of the repo."
    )


class PyPIGitRevRequirement(_PyPiGitRequirement):
    rev: Optional[NonEmptyStr] = Field(None, description="A `git` SHA revision to use")


class PyPIGitBranchRequirement(_PyPiGitRequirement):
    branch: Optional[NonEmptyStr] = Field(None, description="A `git` branch to use")


class PyPIGitTagRequirement(_PyPiGitRequirement):
    tag: Optional[NonEmptyStr] = Field(None, description="A `git` tag to use")


class PyPIPathRequirement(_PyPIRequirement):
    path: NonEmptyStr = Field(
        None,
        description="A path to a local source or wheel",
    )
    editable: Optional[bool] = Field(
        None, description="If `true` the package will be installed as editable"
    )
    subdirectory: NonEmptyStr | None = Field(
        None, description="The subdirectory in the repo, a path from the root of the repo."
    )


class PyPIUrlRequirement(_PyPIRequirement):
    url: NonEmptyStr = Field(
        None,
        description="A URL to a remote source or wheel",
    )


class PyPIVersion(_PyPIRequirement):
    version: NonEmptyStr = Field(
        None,
        description="The version of the package in [PEP 440](https://www.python.org/dev/peps/pep-0440/) format",
    )
    index: NonEmptyStr | None = Field(
        None,
        description="The index to fetch the package from",
    )


PyPIRequirement = (
    NonEmptyStr
    | PyPIVersion
    | PyPIGitBranchRequirement
    | PyPIGitTagRequirement
    | PyPIGitRevRequirement
    | PyPIPathRequirement
    | PyPIUrlRequirement
)
PyPIPackageName = NonEmptyStr

DependenciesField = Field(
    None,
    description="The `conda` dependencies, consisting of a package name and a requirement in [MatchSpec](https://github.com/conda/conda/blob/078e7ee79381060217e1ec7f9b0e9cf80ecc8f3f/conda/models/match_spec.py) format",
)
HostDependenciesField = Field(
    None,
    description="The host `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information.",
    examples=[{"python": ">=3.8"}],
)
BuildDependenciesField = Field(
    None,
    description="The build `conda` dependencies, used in the build process. See https://pixi.sh/latest/build/dependency_types/ for more information.",
)
RunDependenciesField = Field(
    None,
    description="The `conda` dependencies required at runtime. See https://pixi.sh/latest/build/dependency_types/ for more information.",
)
Dependencies = dict[CondaPackageName, MatchSpec] | None

################
# Task section #
################
TaskName = Annotated[str, Field(pattern=r"^[^\s\$]+$", description="A valid task name.")]
TaskArgName = Annotated[
    str, Field(pattern=r"^[a-zA-Z_][a-zA-Z\d_]*$", description="A valid task argument name")
]
TaskArgInlineTable = Annotated[
    dict[TaskArgName, str],
    Field(min_length=1, max_length=1, description="A single item task name/value object"),
]


class TaskArgs(StrictBaseModel):
    """The arguments of a task."""

    arg: TaskArgName = Field(description="The name of the argument")
    default: str | None = Field(None, description="The default value of the argument")


class DependsOn(StrictBaseModel):
    """The dependencies of a task."""

    task: TaskName = Field(description="the name of the task to depend on")
    args: list[str | TaskArgInlineTable] | None = Field(
        None, description="The (positional or named) arguments to pass to the task"
    )
    environment: EnvironmentName | None = Field(
        None, description="The environment to use for the task"
    )


class TaskInlineTable(StrictBaseModel):
    """A precise definition of a task."""

    cmd: list[NonEmptyStr] | NonEmptyStr | None = Field(
        None,
        description="A shell command to run the task in the limited, but cross-platform `bash`-like `deno_task_shell`. See the documentation for [supported syntax](https://pixi.sh/latest/environments/advanced_tasks/#syntax)",
    )
    cwd: PathNoBackslash | None = Field(None, description="The working directory to run the task")
    # BREAK: `depends_on` is deprecated, use `depends-on`
    depends_on_deprecated: list[TaskName] | TaskName | None = Field(
        None,
        alias="depends_on",
        description="The tasks that this task depends on. Environment variables will **not** be expanded. Deprecated in favor of `depends-on` from v0.21.0 onward.",
    )
    depends_on: list[DependsOn | TaskName] | DependsOn | TaskName | None = Field(
        None,
        description="The tasks that this task depends on. Environment variables will **not** be expanded.",
    )
    inputs: list[Glob] | None = Field(
        None,
        description="A list of `.gitignore`-style glob patterns that should be watched for changes before this command is run. Environment variables _will_ be expanded.",
    )
    outputs: list[Glob] | None = Field(
        None,
        description="A list of `.gitignore`-style glob patterns that are generated by this command. Environment variables _will_ be expanded.",
    )
    env: dict[NonEmptyStr, NonEmptyStr] | None = Field(
        None,
        description="A map of environment variables to values, used in the task, these will be overwritten by the shell.",
        examples=[{"key": "value"}, {"ARGUMENT": "value"}],
    )
    description: NonEmptyStr | None = Field(
        None,
        description="A short description of the task",
        examples=["Build the project"],
    )
    clean_env: bool | None = Field(
        None,
        description="Whether to run in a clean environment, removing all environment variables except those defined in `env` and by pixi itself.",
    )
    args: list[TaskArgs | TaskArgName] | None = Field(
        None,
        description="The arguments to a task",
        examples=[
            ["arg1", "arg2"],
            ["arg", {"arg": "arg2", "default": "2"}],
        ],
    )


#######################
# System requirements #
#######################
class LibcFamily(StrictBaseModel):
    family: NonEmptyStr | None = Field(
        None, description="The family of the `libc`", examples=["glibc", "musl"]
    )
    version: float | NonEmptyStr | None = Field(None, description="The version of `libc`")


class SystemRequirements(StrictBaseModel):
    """Platform-specific requirements"""

    linux: PositiveFloat | NonEmptyStr | None = Field(
        None, description="The minimum version of the Linux kernel"
    )
    unix: bool | NonEmptyStr | None = Field(
        None, description="Whether the project supports UNIX", examples=["true"]
    )
    libc: LibcFamily | float | NonEmptyStr | None = Field(
        None, description="The minimum version of `libc`"
    )
    cuda: float | NonEmptyStr | None = Field(None, description="The minimum version of CUDA")
    archspec: NonEmptyStr | None = Field(None, description="The architecture the project supports")
    macos: PositiveFloat | NonEmptyStr | None = Field(
        None, description="The minimum version of MacOS"
    )


#######################
# Environment section #
#######################
EnvironmentName = Annotated[str, Field(pattern=r"^[a-z\d\-]+$")]
FeatureName = NonEmptyStr
SolveGroupName = NonEmptyStr


class Environment(StrictBaseModel):
    """A composition of the dependencies of features which can be activated to run tasks or provide a shell"""

    features: list[FeatureName] | None = Field(
        None, description="The features that define the environment"
    )
    solve_group: SolveGroupName | None = Field(
        None,
        description="The group name for environments that should be solved together",
    )
    no_default_feature: Optional[bool] = Field(
        False,
        description="Whether to add the default feature to this environment",
    )


######################
# Activation section #
######################
class Activation(StrictBaseModel):
    """A description of steps performed when an environment is activated"""

    scripts: list[NonEmptyStr] | None = Field(
        None,
        description="The scripts to run when the environment is activated",
        examples=["activate.sh", "activate.bat"],
    )
    env: dict[NonEmptyStr, NonEmptyStr] | None = Field(
        None,
        description="A map of environment variables to values, used in the activation of the environment. These will be set in the shell. Thus these variables are shell specific. Using '$' might not expand to a value in different shells.",
        examples=[{"key": "value"}, {"ARGUMENT": "value"}],
    )


##################
# Target section #
##################
TargetName = NonEmptyStr


class WorkspaceTarget(StrictBaseModel):
    """Target-specific configuration for a workspace"""

    build_variants: dict[NonEmptyStr, list[str]] | None = Field(
        None, description="The build variants for this workspace target"
    )


class Target(StrictBaseModel):
    """A machine-specific configuration of dependencies and tasks"""

    dependencies: Dependencies = DependenciesField
    host_dependencies: Dependencies = HostDependenciesField
    build_dependencies: Dependencies = BuildDependenciesField
    pypi_dependencies: dict[PyPIPackageName, PyPIRequirement] | None = Field(
        None, description="The PyPI dependencies for this target"
    )
    tasks: dict[TaskName, TaskInlineTable | list[DependsOn] | NonEmptyStr] | None = Field(
        None, description="The tasks of the target"
    )
    activation: Activation | None = Field(
        None, description="The scripts used on the activation of the project for this target"
    )


###################
# Feature section #
###################
class Feature(StrictBaseModel):
    """A composable aspect of the project which can contribute dependencies and tasks to an environment"""

    channels: list[Channel] | None = Field(
        None,
        description="The `conda` channels that can be considered when solving environments containing this feature",
    )
    channel_priority: ChannelPriority | None = Field(
        None,
        examples=["strict", "disabled"],
        description="The type of channel priority that is used in the solve."
        "- 'strict': only take the package from the channel it exist in first."
        "- 'disabled': group all dependencies together as if there is no channel difference.",
    )
    platforms: list[Platform] | None = Field(
        None,
        description="The platforms that the feature supports: a union of all features combined in one environment is used for the environment.",
    )
    dependencies: Dependencies = DependenciesField
    host_dependencies: Dependencies = HostDependenciesField
    build_dependencies: Dependencies = BuildDependenciesField
    pypi_dependencies: dict[PyPIPackageName, PyPIRequirement] | None = Field(
        None, description="The PyPI dependencies of this feature"
    )
    tasks: dict[TaskName, TaskInlineTable | list[DependsOn] | NonEmptyStr] | None = Field(
        None, description="The tasks provided by this feature"
    )
    activation: Activation | None = Field(
        None, description="The scripts used on the activation of environments using this feature"
    )
    system_requirements: SystemRequirements | None = Field(
        None, description="The system requirements of this feature"
    )
    target: dict[TargetName, Target] | None = Field(
        None,
        description="Machine-specific aspects of this feature",
        examples=[{"linux": {"dependencies": {"python": "3.8"}}}],
    )
    pypi_options: PyPIOptions | None = Field(
        None, description="Options related to PyPI indexes for this feature"
    )


###################
# PyPI section #
###################


class FindLinksPath(StrictBaseModel):
    """The path to the directory containing packages"""

    path: NonEmptyStr | None = Field(
        None, description="Path to the directory of packages", examples=["./links"]
    )


class FindLinksURL(StrictBaseModel):
    """The URL to the html file containing href-links to packages"""

    url: NonEmptyStr | None = Field(
        None,
        description="URL to html file with href-links to packages",
        examples=["https://simple-index-is-here.com"],
    )


class S3Options(StrictBaseModel):
    """Options related to S3 for this project"""

    endpoint_url: NonEmptyStr = Field(
        description="The endpoint URL to use for the S3 client",
        examples=["https://s3.eu-central-1.amazonaws.com"],
    )
    region: NonEmptyStr = Field(
        description="The region to use for the S3 client",
        examples=["eu-central-1"],
    )
    force_path_style: bool = Field(
        description="Whether to force path style for the S3 client",
    )


class PyPIOptions(StrictBaseModel):
    """Options that determine the behavior of PyPI package resolution and installation"""

    index_url: NonEmptyStr | None = Field(
        None,
        description="PyPI registry that should be used as the primary index",
        examples=["https://pypi.org/simple"],
    )
    extra_index_urls: list[NonEmptyStr] | None = Field(
        None,
        description="Additional PyPI registries that should be used as extra indexes",
        examples=[["https://pypi.org/simple"]],
    )
    find_links: list[FindLinksPath | FindLinksURL] = Field(
        None,
        description="Paths to directory containing",
        examples=[["https://pypi.org/simple"]],
    )
    no_build_isolation: bool | list[PyPIPackageName] | None = Field(
        None,
        description="Packages that should NOT be isolated during the build process",
        examples=[["numpy"], True],
    )
    index_strategy: (
        Literal["first-index"] | Literal["unsafe-first-match"] | Literal["unsafe-best-match"] | None
    ) = Field(
        None,
        description="The strategy to use when resolving packages from multiple indexes",
        examples=["first-index", "unsafe-first-match", "unsafe-best-match"],
    )
    no_build: bool | list[PyPIPackageName] | None = Field(
        None,
        description="Packages that should NOT be built",
        examples=["true", "false"],
    )
    dependency_overrides: dict[PyPIPackageName, PyPIRequirement] | None = Field(
        None,
        description="A list of PyPI dependencies that override the resolved dependencies",
        examples=[
            {"numpy": ">=1.21.0"},
        ],
    )
    no_binary: bool | list[PyPIPackageName] | None = Field(
        None,
        description="Don't use pre-built wheels for these packages",
        examples=["true", "false"],
    )


#######################
# The Package section #
#######################


class Package(StrictBaseModel):
    """The package's metadata information."""

    name: NonEmptyStr | WorkspaceInheritance | None = Field(
        None,
        description="The name of the package. Can be a string or { workspace = true } to inherit from workspace",
    )
    version: NonEmptyStr | WorkspaceInheritance | None = Field(
        None,
        description="The version of the project; we advise use of [SemVer](https://semver.org). Can be a string or { workspace = true } to inherit from workspace",
        examples=["1.2.3", {"workspace": True}],
    )
    description: NonEmptyStr | WorkspaceInheritance | None = Field(
        None,
        description="A short description of the project. Can be a string or { workspace = true } to inherit from workspace",
    )
    authors: list[NonEmptyStr] | WorkspaceInheritance | None = Field(
        None,
        description="The authors of the project. Can be a list of strings or { workspace = true } to inherit from workspace",
        examples=[["John Doe <j.doe@prefix.dev>"], {"workspace": True}],
    )
    license: NonEmptyStr | WorkspaceInheritance | None = Field(
        None,
        description="The license of the project; we advise using an [SPDX](https://spdx.org/licenses/) identifier. Can be a string or { workspace = true } to inherit from workspace",
    )
    license_file: PathNoBackslash | WorkspaceInheritance | None = Field(
        None,
        description="The path to the license file of the project. Can be a path or { workspace = true } to inherit from workspace",
    )
    readme: PathNoBackslash | WorkspaceInheritance | None = Field(
        None,
        description="The path to the readme file of the project. Can be a path or { workspace = true } to inherit from workspace",
    )
    homepage: AnyHttpUrl | WorkspaceInheritance | None = Field(
        None,
        description="The URL of the homepage of the project. Can be a URL or { workspace = true } to inherit from workspace",
    )
    repository: AnyHttpUrl | WorkspaceInheritance | None = Field(
        None,
        description="The URL of the repository of the project. Can be a URL or { workspace = true } to inherit from workspace",
    )
    documentation: AnyHttpUrl | WorkspaceInheritance | None = Field(
        None,
        description="The URL of the documentation of the project. Can be a URL or { workspace = true } to inherit from workspace",
    )

    build: Build = Field(..., description="The build configuration of the package")

    host_dependencies: Dependencies = HostDependenciesField
    build_dependencies: Dependencies = BuildDependenciesField
    run_dependencies: Dependencies = RunDependenciesField

    target: dict[TargetName, Target] | None = Field(
        None,
        description="Machine-specific aspects of the package",
        examples=[{"linux": {"host-dependencies": {"python": "3.8"}}}],
    )


class BuildTarget(StrictBaseModel):
    """Target-specific build configuration for different platforms"""

    config: dict[str, Any] = Field(
        None, description="Target-specific configuration for the build backend"
    )


class SourceLocation(StrictBaseModel):
    """The location of a package's source code."""

    path: NonEmptyStr | None = Field(None, description="The path to the source")

    # TODO: url and git source
    # url: NonEmptyStr | None = Field(None, description="The URL to the source")
    # md5: Md5Sum | None = Field(None, description="The md5 hash of the source")
    # sha256: Sha256Sum | None = Field(None, description="The sha256 hash of the source")

    # git: NonEmptyStr | None = Field(None, description="The git URL to the source repo")
    # rev: NonEmptyStr | None = Field(None, description="A git SHA revision to use")
    # tag: NonEmptyStr | None = Field(None, description="A git tag to use")
    # branch: NonEmptyStr | None = Field(None, description="A git branch to use")
    # subdirectory: NonEmptyStr | None = Field(None, description="A subdirectory to use in the repo")


class Build(StrictBaseModel):
    backend: BuildBackend = Field(..., description="The build backend to instantiate")
    channels: list[Channel] = Field(
        None, description="The `conda` channels that are used to fetch the build backend from"
    )
    additional_dependencies: Dependencies = Field(
        None, description="Additional dependencies to install alongside the build backend"
    )
    config: dict[str, Any] = Field(None, description="The configuration of the build backend")
    target: dict[TargetName, BuildTarget] | None = Field(
        None,
        description="Target-specific build configuration for different platforms",
        examples=[{"linux-64": {"config": {"key": "value"}}}],
    )
    source: SourceLocation = Field(
        None,
        description="The source from which to build the package",
        examples=[{"path": "project"}],
    )


class BuildBackend(MatchspecTable):
    name: NonEmptyStr = Field(None, description="The name of the build backend package")
    channels: list[Channel] | None = Field(
        None, description="The `conda` channels that are used to fetch the build backend from"
    )
    additional_dependencies: Dependencies = Field(
        None, description="Additional dependencies to install alongside the build backend"
    )


class PackageTarget(StrictBaseModel):
    run_dependencies: Dependencies = RunDependenciesField
    host_dependencies: Dependencies = HostDependenciesField
    build_dependencies: Dependencies = BuildDependenciesField


#######################
# The Manifest itself #
#######################


class BaseManifest(StrictBaseModel):
    """The configuration for a [`pixi`](https://pixi.sh) project."""

    class Config:
        json_schema_extra = {
            "$id": SCHEMA_URI,
            "$schema": SCHEMA_DRAFT,
            "title": "`pixi.toml` manifest file",
            "anyOf": [
                {"required": ["project"]},
                {"required": ["workspace"]},
                {"required": ["package"]},
            ],
        }

    schema_: str | None = Field(
        SCHEMA_URI,
        alias="$schema",
        title="Schema",
        description="The schema identifier for the project's configuration",
        format="uri-reference",
    )

    workspace: Workspace | None = Field(None, description="The workspace's metadata information")
    project: Workspace | None = Field(None, description="The project's metadata information")
    package: Package | None = Field(None, description="The package's metadata information")
    dependencies: Dependencies = DependenciesField
    host_dependencies: Dependencies = HostDependenciesField
    build_dependencies: Dependencies = BuildDependenciesField
    pypi_dependencies: dict[PyPIPackageName, PyPIRequirement] | None = Field(
        None, description="The PyPI dependencies"
    )
    pypi_options: PyPIOptions | None = Field(None, description="Options related to PyPI indexes")
    tasks: dict[TaskName, TaskInlineTable | list[DependsOn] | NonEmptyStr] | None = Field(
        None, description="The tasks of the project"
    )
    system_requirements: SystemRequirements | None = Field(
        None, description="The system requirements of the project"
    )
    environments: dict[EnvironmentName, Environment | list[FeatureName]] | None = Field(
        None,
        description="The environments of the project, defined as a full object or a list of feature names.",
    )
    feature: dict[FeatureName, Feature] | None = Field(
        None, description="The features of the project"
    )
    activation: Activation | None = Field(
        None, description="The scripts used on the activation of the project"
    )
    target: dict[TargetName, Target] | None = Field(
        None,
        description="The targets of the project",
        examples=[{"linux": {"dependencies": {"python": "3.8"}}}],
    )
    tool: dict[str, Any] = Field(
        None, description="Third-party tool configurations, ignored by pixi"
    )
    pypi_options: PyPIOptions | None = Field(
        None,
        description="Options related to PyPI indexes, on the default feature",
    )


#########################
# JSON Schema utilities #
#########################


class SchemaJsonEncoder(json.JSONEncoder):
    """A custom schema encoder for normalizing schema to be used with TOML files."""

    HEADER_ORDER = [
        "$schema",
        "$id",
        "$ref",
        "title",
        "deprecated",
        "description",
        "type",
        "required",
        "additionalProperties",
        "default",
        "items",
        "properties",
        "patternProperties",
        "allOf",
        "anyOf",
        "oneOf",
        "not",
        "format",
        "minimum",
        "exclusiveMinimum",
        "maximum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "multipleOf",
        "pattern",
    ]
    FOOTER_ORDER = [
        "examples",
        "$defs",
    ]
    SORT_NESTED = [
        "items",
    ]
    SORT_NESTED_OBJ = [
        "properties",
        "$defs",
    ]
    SORT_NESTED_MAYBE_OBJ = [
        "additionalProperties",
    ]
    SORT_NESTED_OBJ_OBJ = [
        "patternProperties",
    ]
    SORT_NESTED_ARR = [
        "anyOf",
        "allOf",
        "oneOf",
    ]

    def encode(self, obj):
        """Overload the default ``encode`` behavior."""
        if isinstance(obj, dict):
            obj = self.normalize_schema(deepcopy(obj))

        return super().encode(obj)

    def normalize_schema(self, obj: dict[str, Any]) -> dict[str, Any]:
        """Recursively normalize and apply an arbitrary sort order to a schema."""
        self.strip_nulls(obj)

        for nest in self.SORT_NESTED:
            if nest in obj:
                obj[nest] = self.normalize_schema(obj[nest])

        for nest in self.SORT_NESTED_OBJ:
            obj = self.sort_nested(obj, nest)

        for nest in self.SORT_NESTED_OBJ_OBJ:
            if nest in obj:
                obj[nest] = {
                    k: self.normalize_schema(v)
                    for k, v in sorted(obj[nest].items(), key=lambda kv: kv[0])
                }

        for nest in self.SORT_NESTED_ARR:
            if nest in obj:
                obj[nest] = [self.normalize_schema(item) for item in obj[nest]]

        for nest in self.SORT_NESTED_MAYBE_OBJ:
            if isinstance(obj.get(nest), dict):
                obj[nest] = self.normalize_schema(obj[nest])

        header = {}
        footer = {}

        for key in self.HEADER_ORDER:
            if key in obj:
                header[key] = obj.pop(key)

        for key in self.FOOTER_ORDER:
            if key in obj:
                footer[key] = obj.pop(key)

        return {**header, **dict(sorted(obj.items())), **footer}

    def strip_nulls(self, obj: dict[str, Any]) -> dict[str, Any]:
        """Remove unrepresentable-in-TOML ``"anyOf":{"type": null}`` values."""

        if "default" in obj and obj["default"] is None:
            obj.pop("default")

        for nest in self.SORT_NESTED_ARR:
            some_of = [
                self.normalize_schema(option)
                for option in obj.get(nest, [])
                if option.get("type") != "null"
            ]

            if some_of:
                obj[nest] = some_of
                if len(some_of) == 1:
                    obj.update(some_of[0])
                    obj.pop(nest)

        return obj

    def sort_nested(self, obj: dict[str, Any], key: str) -> dict[str, Any]:
        """Sort a key of an object."""
        if key not in obj or not isinstance(obj[key], dict):
            return obj
        obj[key] = {
            k: self.normalize_schema(v) if isinstance(v, dict) else v
            for k, v in sorted(obj[key].items(), key=lambda kv: kv[0])
        }
        return obj


##########################
# Command Line Interface #
##########################

if __name__ == "__main__":
    print(json.dumps(BaseManifest.model_json_schema(), indent=2, cls=SchemaJsonEncoder))
