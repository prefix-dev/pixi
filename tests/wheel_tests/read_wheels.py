import tomllib

from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Self, Any
from collections.abc import Iterable


@dataclass
class PackageSpec:
    """
    A class to represent the package specification in the `wheels.toml` file.
    A single Package Name can have multiple specifications.
    """

    version: Literal["*"] | str = "*"
    extras: str | None = None
    target: str | list[str] | None = None
    system_requirements: dict[str, Any] | None = None

    def target_iter(self) -> Iterable[str]:
        """
        Returns an iterable of the target platforms
        """
        if isinstance(self.target, str):
            return [self.target]
        elif isinstance(self.target, list):
            return self.target
        return []

    @classmethod
    def __from_toml(cls, spec: dict[str, Any] | str) -> Self:
        if isinstance(spec, str):
            return cls(version=spec, extras=None, target=None, system_requirements=None)
        if isinstance(spec, dict):
            return cls(
                spec.get("version", "*"),
                spec.get("extras"),
                spec.get("target"),
                spec.get("system-requirements"),
            )

    @classmethod
    def from_toml(cls, spec: dict[str, str] | list[dict[str, str]] | str) -> Self | list[Self]:
        if isinstance(spec, list):
            return [cls.__from_toml(s) for s in spec]
        else:
            return cls.__from_toml(spec)


@dataclass
class Package:
    """
    Specifies a package which is a name and a specification
    on how to install it.
    """

    # Name of the package
    name: str
    # Specification of the package
    spec: PackageSpec

    def to_add_cmd(self) -> str:
        """
        Converts the package to a command that can be consumed with the
        `pixi add` command.
        """
        cmd = f"{self.name}"
        if self.spec.extras:
            cmd = f"{cmd}[{self.spec.extras}]"
        if self.spec.version and self.spec.version != "*":
            cmd = f"{cmd}=={self.spec.version}"
        return cmd


@dataclass
class WheelTest:
    """
    A class to represent the `wheels.toml` file
    """

    # Mapping of wheel names to installation specifications
    name: dict[str, list[PackageSpec] | PackageSpec]

    def to_packages(self) -> Iterable[Package]:
        """
        Converts to a list of installable packages
        """
        for name, specs in self.name.items():
            if isinstance(specs, PackageSpec):
                yield Package(name, specs)
            else:
                yield from [Package(name, spec) for spec in specs]

    @classmethod
    def from_toml(cls, file: Path) -> Self:
        """
        Read the wheels from the toml file and return the instance
        """
        with file.open("rb") as f:
            toml = tomllib.load(f)
            if not isinstance(toml, dict):
                raise ValueError("Expected a dictionary")
            wheels = toml
            return cls({name: PackageSpec.from_toml(spec) for name, spec in wheels.items()})

    @classmethod
    def from_str(cls, s: str) -> Self:
        """
        Read the wheels from the toml string and return the instance
        """
        toml = tomllib.loads(s)
        return cls({name: PackageSpec.from_toml(spec) for name, spec in toml.items()})


def read_wheel_file() -> Iterable[Package]:
    """
    Read the wheel file `wheels.toml` and return the package
    instances.
    """
    wheel_path = Path(__file__).parent / Path("wheels.toml")
    return WheelTest.from_toml(wheel_path).to_packages()
