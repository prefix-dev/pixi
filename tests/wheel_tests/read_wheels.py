from dataclasses import dataclass
from pathlib import Path
import tomllib
from typing import Literal, Iterable, Self


@dataclass
class PackageSpec:
    version: Literal["*"] | str = "*"
    extras: str | None = None
    target: str | None = None

    @classmethod
    def __from_toml(cls, spec: dict[str, str] | str) -> Self:
        if isinstance(spec, str):
            return cls(version=spec, extras=None, target=None)
        if isinstance(spec, dict):
            return cls(spec.get("version", "*"), spec.get("extras"), spec.get("target"))

    @classmethod
    def from_toml(cls, spec: dict[str, str] | list[dict[str, str]] | str) -> Self | list[Self]:
        if isinstance(spec, list):
            return [cls.__from_toml(s) for s in spec]
        else:
            return cls.__from_toml(spec)


@dataclass
class Package:
    name: str
    spec: PackageSpec

    def to_add_cmd(self) -> str:
        cmd = f"{self.name}"
        if self.spec.extras:
            cmd = f"{cmd}[{self.spec.extras}]"
        if self.spec.version and self.spec.version != "*":
            cmd = f"{cmd}=={self.spec.version}"
        return cmd


@dataclass
class WheelTest:
    name: dict[str, list[PackageSpec] | PackageSpec]

    def to_packages(self) -> Iterable[Package]:
        for name, specs in self.name.items():
            if isinstance(specs, PackageSpec):
                yield Package(name, specs)
            else:
                yield from [Package(name, spec) for spec in specs]

    @classmethod
    def from_toml(cls, file: Path) -> Self:
        with file.open("rb") as f:
            toml = tomllib.load(f)
            if not isinstance(toml, dict):
                raise ValueError("Expected a dictionary")
            if "wheels" not in toml:
                raise ValueError("Expected a 'wheels' key")
            wheels = toml["wheels"]
            return cls({name: PackageSpec.from_toml(spec) for name, spec in wheels.items()})

    @classmethod
    def from_str(cls, s: str) -> Self:
        toml = tomllib.loads(s)
        return cls({name: PackageSpec.from_toml(spec) for name, spec in toml.items()})
