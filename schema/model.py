"""A canonical schema definition for the ``pixi.toml`` manifest file."""
from __future__ import annotations

import json
from copy import deepcopy
from pathlib import Path
import tomllib
from typing import Annotated, Any, Optional, Literal

from pydantic import (
    AnyHttpUrl,
    BaseModel,
    Field,
    PositiveFloat,
    StringConstraints,
)

#: latest version currently supported by the `taplo` TOML linter and language server
SCHEMA_DRAFT = "http://json-schema.org/draft-07/schema#"
CARGO_TOML = Path(__file__).parent.parent / "Cargo.toml"
CARGO_TOML_DATA = tomllib.loads(CARGO_TOML.read_text(encoding="utf-8"))
VERSION = CARGO_TOML_DATA["package"]["version"]
SCHEMA_URI = f"https://pixi.sh/v{VERSION}/schema/manifest/schema.json"


NonEmptyStr = Annotated[str, StringConstraints(min_length=1)]
PathNoBackslash = Annotated[str, StringConstraints(pattern=r"^[^\\]+$")]
Glob = NonEmptyStr
UnsignedInt = Annotated[int, Field(strict=True, ge=0)]
GitUrl = Annotated[
    str, StringConstraints(pattern=r"((git|ssh|http(s)?)|(git@[\w\.]+))(:(\/\/)?)([\w\.@:\/\\-~]+)")
]
Platform = (
    Literal["linux-32"]
    | Literal["linux-64"]
    | Literal["linux-aarch64"]
    | Literal["linux-armv6l"]
    | Literal["linux-armv7l"]
    | Literal["linux-ppc64le"]
    | Literal["linux-ppc64"]
    | Literal["linux-s390x"]
    | Literal["linux-riscv32"]
    | Literal["linux-riscv64"]
    | Literal["osx-64"]
    | Literal["osx-arm64"]
    | Literal["win-32"]
    | Literal["win-64"]
    | Literal["win-arm64"]
)


class StrictBaseModel(BaseModel):
    class Config:
        extra = "forbid"


###################
# Project section #
###################
ChannelName = NonEmptyStr | AnyHttpUrl


class ChannelInlineTable(StrictBaseModel):
    """A precise description of a `conda` channel, with an optional priority."""
    channel: ChannelName = Field(description="The channel the packages needs to be fetched from")
    priority: int | None = Field(None, description="The priority of the channel")


Channel = NonEmptyStr | ChannelInlineTable


class Project(StrictBaseModel):
    """The project's metadata information."""
    name: NonEmptyStr = Field(
        description="The name of the project; we advise use of the name of the repository"
    )
    version: NonEmptyStr | None = Field(
        None, description="The version of the project; we advise use of [SemVer](https://semver.org)", examples=["1.2.3"]
    )
    description: NonEmptyStr | None = Field(None, description="A short description of the project")
    authors: list[NonEmptyStr] | None = Field(
        None, description="The authors of the project", examples=["John Doe <j.doe@prefix.dev>"]
    )
    channels: list[Channel] = Field(
        None, description="The `conda` channels that can be used in the project. Unless overridden by `priority`, the first channel listed will be preferred."
    )
    platforms: list[Platform] = Field(description="The platforms that the project supports")
    license: NonEmptyStr | None = Field(None, description="The license of the project; we advise using an [SPDX](https://spdx.org/licenses/) identifier.")
    license_file: PathNoBackslash | None = Field(
        None, alias="license-file", description="The path to the license file of the project"
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
        None, alias="conda-pypi-map", description="The `conda` to PyPI mapping configuration"
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
    channel: NonEmptyStr | None = Field(
        None,
        description="The channel the packages needs to be fetched from",
        examples=["conda-forge", "pytorch", "https://repo.prefix.dev/conda-forge"],
    )


MatchSpec = NonEmptyStr | MatchspecTable
CondaPackageName = NonEmptyStr


# { version = "sdfds" extras = ["sdf"] }
# { git = "sfds", rev = "fssd" }
# { path = "asfdsf" }
# { url = "asdfs" }


class _PyPIRequirement(StrictBaseModel):
    extras: list[NonEmptyStr] | None = Field(None, description="The [PEP 508 extras](https://peps.python.org/pep-0508/#extras) of the package")


class _PyPiGitRequirement(_PyPIRequirement):
    git: NonEmptyStr = Field(
        None,
        description="The `git` URL to the repo e.g https://github.com/prefix-dev/pixi",
    )


class PyPIGitRevRequirement(_PyPiGitRequirement):
    rev: Optional[NonEmptyStr] = Field(None, description="A `git` SHA revision to sue")


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
    alias="host-dependencies",
    description="The host `conda` dependencies, used in the build process", examples=[{"python": ">=3.8"}]
)
BuildDependenciesField = Field(
    None,
    alias="build-dependencies",
    description="The build `conda` dependencies, used in the build process",
)
Dependencies = dict[CondaPackageName, MatchSpec] | None

################
# Task section #
################
TaskName = Annotated[str, Field(pattern=r"^[^\s\$]+$", description="A valid task name.")]

class TaskInlineTable(StrictBaseModel):
    """A precise definition of a task."""
    cmd: list[NonEmptyStr] | NonEmptyStr | None = Field(
        None, description="A shell command to run the task in the limited, but cross-platform `bash`-like `deno_task_shell`. See the documentation for [supported syntax](https://pixi.sh/latest/features/advanced_tasks/#syntax)"
    )
    cwd: PathNoBackslash | None = Field(None, description="The working directory to run the task")
    depends_on: list[TaskName] | TaskName | None = Field(
        None, description="The tasks that this task depends on. Environment variables will **not** be expanded."
    )
    inputs: list[Glob] | None = Field(
        None,
        description="A list of `.gitignore`-style glob patterns that should be watched for changes before this command is run. Environment variables _will_ be expanded.",
    )
    outputs: list[Glob] | None = Field(
        None, description="A list of `.gitignore`-style glob patterns that are generated by this command. Environment variables _will_ be expanded."
    )
    env: dict[NonEmptyStr, NonEmptyStr] | None = Field(
        None, description="A map of environment variables to values, used in the task, these will be overwritten by the shell." , examples = [{"key": "value"}, {"ARGUMENT": "value"}]
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
        alias="solve-group",
        description="The group name for environments that should be solved together",
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


##################
# Target section #
##################
TargetName = NonEmptyStr


class Target(StrictBaseModel):
    """A machine-specific configuration of dependencies and tasks"""
    dependencies: Dependencies = DependenciesField
    host_dependencies: Dependencies = HostDependenciesField
    build_dependencies: Dependencies = BuildDependenciesField
    pypi_dependencies: dict[PyPIPackageName, PyPIRequirement] | None = Field(
        None, alias="pypi-dependencies", description="The PyPI dependencies for this target"
    )
    tasks: dict[TaskName, TaskInlineTable | NonEmptyStr] | None = Field(
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
        None, description="The `conda` channels that can be considered when solving environments containing this feature"
    )
    platforms: list[NonEmptyStr] | None = Field(
        None,
        description="The platforms that the feature supports: a union of all features combined in one environment is used for the environment.",
    )
    dependencies: Dependencies = DependenciesField
    host_dependencies: Dependencies = HostDependenciesField
    build_dependencies: Dependencies = BuildDependenciesField
    pypi_dependencies: dict[PyPIPackageName, PyPIRequirement] | None = Field(
        None, alias="pypi-dependencies", description="The PyPI dependencies of this feature"
    )
    tasks: dict[TaskName, TaskInlineTable | NonEmptyStr] | None = Field(
        None, description="The tasks provided by this feature"
    )
    activation: Activation | None = Field(
        None, description="The scripts used on the activation of environments using this feature"
    )
    system_requirements: SystemRequirements | None = Field(
        None, alias="system-requirements", description="The system requirements of this feature"
    )
    target: dict[TargetName, Target] | None = Field(
        None,
        description="Machine-specific aspects of this feature",
        examples=[{"linux": {"dependencies": {"python": "3.8"}}}],
    )
    pypi_options: PyPIOptions | None = Field(None, alias="pypi-options", description="Options related to PyPI indexes for this feature")

###################
# PyPI section #
###################

class FindLinksPath(StrictBaseModel):
    """The path to the directory containing packages"""
    path: NonEmptyStr | None = Field(None, description="Path to the directory of packages", examples=["./links"])

class FindLinksURL(StrictBaseModel):
    """The URL to the html file containing href-links to packages"""
    url: NonEmptyStr | None = Field(None, description="URL to html file with href-links to packages", examples=["https://simple-index-is-here.com"])

class PyPIOptions(StrictBaseModel):
    """Options related to PyPI indexes"""
    index_url: NonEmptyStr | None = Field(None, alias="index-url", description="Alternative PyPI registry that should be used as the main index", examples=["https://pypi.org/simple"])
    extra_index_urls: list[NonEmptyStr] | None = Field(None, alias="extra-index-urls", description="Additional PyPI registries that should be used as extra indexes", examples=[["https://pypi.org/simple"]])
    find_links: list[FindLinksPath | FindLinksURL] = Field(None, alias="find-links", description="Paths to directory containing", examples=[["https://pypi.org/simple"]])

#######################
# The Manifest itself #
#######################


class BaseManifest(StrictBaseModel):
    """The configuration for a [`pixi`](https://pixi.sh) project."""
    class Config:
        json_schema_extra = {
            "$id": SCHEMA_URI,
            "$schema": SCHEMA_DRAFT,
            "title": "`pixi.toml` manifest file"
        }

    schema_: str | None = Field(
        SCHEMA_URI,
        alias="$schema",
        title="Schema",
        description="The schema identifier for the project's configuration",
        format="uri-reference"
    )

    project: Project = Field(..., description="The project's metadata information")
    dependencies: Dependencies = DependenciesField
    host_dependencies: Dependencies = HostDependenciesField
    build_dependencies: Dependencies = BuildDependenciesField
    pypi_dependencies: dict[PyPIPackageName, PyPIRequirement] | None = Field(
        None, alias="pypi-dependencies", description="The PyPI dependencies"
    )
    pypi_options: PyPIOptions | None = Field(None, alias="pypi-options", description="Options related to PyPI indexes")
    tasks: dict[TaskName, TaskInlineTable | NonEmptyStr] | None = Field(
        None, description="The tasks of the project"
    )
    system_requirements: SystemRequirements | None = Field(
        None, alias="system-requirements", description="The system requirements of the project"
    )
    environments: dict[EnvironmentName, Environment | list[FeatureName]] | None = Field(
        None, description="The environments of the project, defined as a full object or a list of feature names."
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
    tool: dict[str, Any] = Field(None, description="Third-party tool configurations, ignored by pixi")


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
        "items"
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
                self.normalize_schema(option) for option in obj.get(nest, [])
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
            for k, v in sorted(
                obj[key].items(),
                key=lambda kv: kv[0]
            )
        }
        return obj

##########################
# Command Line Interface #
##########################

if __name__ == "__main__":
    print(
        json.dumps(
            BaseManifest.model_json_schema(),
            indent=2,
            cls=SchemaJsonEncoder
        )
    )
