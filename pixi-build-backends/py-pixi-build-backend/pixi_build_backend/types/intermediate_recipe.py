from typing import Optional, List, Dict, Union
from pathlib import Path
from pixi_build_backend.pixi_build_backend import (
    PyIntermediateRecipe,
    PyPackage,
    PyBuild,
    PyConditionalRequirements,
    PyAbout,
    PyExtra,
    PyScript,
    PyPython,
    PyNoArchKind,
    PyValueString,
    PyValueU64,
    PySource,
    PyUrlSource,
    PyPathSource,
    PyPackageSpecDependencies,
    PyItemString,
)
from pixi_build_backend.types.item import VecItemPackageDependency, ItemPackageDependency
from pixi_build_backend.types.platform import Platform


ConditionalListPackageDependency = List["ItemPackageDependency"]
ConditionalListString = List["ItemString"]


class IntermediateRecipe:
    """An intermediate recipe wrapper."""

    _inner: PyIntermediateRecipe

    def __init__(self) -> None:
        self._inner = PyIntermediateRecipe()

    @property
    def package(self) -> "Package":
        """Get the package information."""
        return Package._from_inner(self._inner.package)

    @package.setter
    def package(self, value: "Package") -> None:
        """Set the package information."""
        self._inner.package = value._inner

    @property
    def build(self) -> "Build":
        """Get the build configuration."""
        return Build._from_inner(self._inner.build)

    @build.setter
    def build(self, value: "Build") -> None:
        """Set the build configuration."""
        self._inner.build = value._inner

    @property
    def requirements(self) -> "ConditionalRequirements":
        """Get the requirements configuration."""
        return ConditionalRequirements._from_inner(self._inner.requirements)

    @requirements.setter
    def requirements(self, value: "ConditionalRequirements") -> None:
        """Set the requirements configuration."""
        self._inner.requirements = value._inner

    @property
    def about(self) -> Optional["About"]:
        """Get the about information."""
        inner_about = self._inner.about
        return About._from_inner(inner_about) if inner_about else None

    @property
    def extra(self) -> Optional["Extra"]:
        """Get the extra information."""
        inner_extra = self._inner.extra
        return Extra._from_inner(inner_extra) if inner_extra else None

    def __repr__(self) -> str:
        return self._inner.__repr__()

    @classmethod
    def _from_inner(cls, inner: PyIntermediateRecipe) -> "IntermediateRecipe":
        """Create an IntermediateRecipe from a PyIntermediateRecipe."""
        instance = cls()
        instance._inner = inner
        return instance

    @staticmethod
    def from_yaml(yaml: str) -> "IntermediateRecipe":
        """
        Create an IntermediateRecipe from a YAML string.

        Parameters
        ----------
        yaml : str
            The YAML string representing the recipe.

        Returns
        -------
        IntermediateRecipe
            The constructed IntermediateRecipe object.

        Examples
        --------
        ```python
        >>> yaml_str = "package:\\n  name: test\\n  version: 1.0.0"
        >>> recipe = IntermediateRecipe.from_yaml(yaml_str)
        >>> recipe.package.name.get_concrete()
        'test'
        >>>
        ```
        """
        return IntermediateRecipe._from_inner(PyIntermediateRecipe.from_yaml(yaml))

    def to_yaml(self) -> str:
        """
        Convert the IntermediateRecipe to a YAML string.

        Returns
        -------
        str
            The YAML representation of the IntermediateRecipe.

        Examples
        --------
        ```python
        >>> recipe = IntermediateRecipe()
        >>> yaml_output = recipe.to_yaml()
        >>> isinstance(yaml_output, str)
        True
        >>>
        ```
        """
        return self._inner.to_yaml()

    def __str__(self) -> str:
        """
        Get the string representation of the IntermediateRecipe.

        Returns
        -------
        str
            The YAML representation of the IntermediateRecipe.

        """
        return str(self._inner)


class Package:
    """A package wrapper."""

    _inner: PyPackage

    def __init__(self, name: str, version: str):
        self._inner = PyPackage(ValueString.concrete(name)._inner, ValueString.concrete(version)._inner)

    @property
    def name(self) -> "ValueString":
        """Get the package name."""
        return ValueString._from_inner(self._inner.name)

    @name.setter
    def name(self, value: str) -> None:
        """Set the package name."""
        self._inner.name = ValueString(value)._inner

    @property
    def version(self) -> "ValueString":
        """Get the package version."""
        return ValueString._from_inner(self._inner.version)

    @version.setter
    def version(self, value: "ValueString") -> None:
        """Set the package version."""
        self._inner.version = value._inner

    @classmethod
    def _from_inner(cls, inner: PyPackage) -> "Package":
        """Create a Package from a PyPackage."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance


class Build:
    """A build configuration wrapper."""

    _inner: PyBuild

    def __init__(self) -> None:
        self._inner = PyBuild()

    @property
    def number(self) -> Optional["ValueU64"]:
        """Get the build number."""
        inner_number = self._inner.number
        return ValueU64._from_inner(inner_number) if inner_number else None

    @number.setter
    def number(self, value: Optional["ValueU64"]) -> None:
        """Set the build number."""
        self._inner.number = value._inner if value else None

    @property
    def script(self) -> "Script":
        """Get the build script."""
        return Script._from_inner(self._inner.script)

    @script.setter
    def script(self, value: "Script") -> None:
        """Set the build script."""
        self._inner.script = value._inner

    @property
    def noarch(self) -> Optional["NoArchKind"]:
        """Get the noarch kind."""
        inner_noarch = self._inner.noarch
        return NoArchKind._from_inner(inner_noarch) if inner_noarch else None

    @noarch.setter
    def noarch(self, value: Optional["NoArchKind"]) -> None:
        """Set the noarch kind."""
        self._inner.noarch = value._inner if value else None

    @property
    def python(self) -> "Python":
        """Get the Python configuration."""
        return Python._from_inner(self._inner.python)

    @python.setter
    def python(self, value: "Python") -> None:
        """Set the Python configuration."""
        self._inner.python = value._inner

    @classmethod
    def _from_inner(cls, inner: PyBuild) -> "Build":
        """Create a Build from a PyBuild."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    def __str__(self) -> str:
        """
        Get the string representation of the Build configuration.

        Returns
        -------
        str
            The string representation of the build configuration.
        """
        return str(self._inner)


class Script:
    """A script wrapper."""

    _inner: PyScript

    def __init__(self, content: Union[str, List[str]], env: Optional[Dict[str, str]] = None):
        # Convert to string for internal storage
        if isinstance(content, list):
            content_str = "\n".join(content)
        else:
            content_str = content if content else ""
        self._inner = PyScript(content_str, env, None)

    @property
    def content(self) -> List[str]:
        """Get the script content."""
        # Convert string back to list for Python API
        content_str = self._inner.content
        if not content_str:
            return []
        return content_str.split("\n")

    @content.setter
    def content(self, value: Union[str, List[str]]) -> None:
        """Set the script content."""
        if isinstance(value, str):
            self._inner.content = value
        else:
            self._inner.content = "\n".join(value)

    @property
    def env(self) -> Dict[str, str]:
        """Get the environment variables."""
        return self._inner.env

    @env.setter
    def env(self, value: Dict[str, str]) -> None:
        """Set the environment variables."""
        self._inner.env = value

    @property
    def secrets(self) -> List[str]:
        """Get the secrets."""
        return self._inner.secrets

    @secrets.setter
    def secrets(self, value: List[str]) -> None:
        """Set the secrets."""
        self._inner.secrets = value

    @classmethod
    def _from_inner(cls, inner: PyScript) -> "Script":
        """Create a Script from a PyScript."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance


class Python:
    """A Python configuration wrapper."""

    _inner: PyPython

    def __init__(self, entry_points: List[str]):
        self._inner = PyPython(entry_points)

    @property
    def entry_points(self) -> List[str]:
        """Get the entry points."""
        return self._inner.entry_points

    @entry_points.setter
    def entry_points(self, value: List[str]) -> None:
        """Set the entry points."""
        self._inner.set_entry_points(value)

    @classmethod
    def _from_inner(cls, inner: PyPython) -> "Python":
        """Create a Python from a PyPython."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    def __str__(self) -> str:
        """
        Get the string representation of the Python configuration.

        Returns
        -------
        str
            The string representation of the entry points.
        """
        return str(self._inner)


class NoArchKind:
    """A NoArch kind wrapper."""

    _inner: PyNoArchKind

    @classmethod
    def python(cls) -> "NoArchKind":
        """
        Create a Python NoArch kind.

        Examples
        --------
        ```python
        >>> kind = NoArchKind.python()
        >>> kind.is_python()
        True
        >>> kind.is_generic()
        False
        >>>
        ```
        """
        instance = cls.__new__(cls)
        instance._inner = PyNoArchKind.python()
        return instance

    @classmethod
    def generic(cls) -> "NoArchKind":
        """
        Create a Generic NoArch kind.

        Examples
        --------
        ```python
        >>> kind = NoArchKind.generic()
        >>> kind.is_generic()
        True
        >>> kind.is_python()
        False
        >>>
        ```
        """
        instance = cls.__new__(cls)
        instance._inner = PyNoArchKind.generic()
        return instance

    def is_python(self) -> bool:
        """
        Check if this is a Python NoArch kind.

        Examples
        --------
        ```python
        >>> NoArchKind.python().is_python()
        True
        >>> NoArchKind.generic().is_python()
        False
        >>>
        ```
        """
        return self._inner.is_python()

    def is_generic(self) -> bool:
        """
        Check if this is a Generic NoArch kind.

        Examples
        --------
        ```python
        >>> NoArchKind.generic().is_generic()
        True
        >>> NoArchKind.python().is_generic()
        False
        >>>
        ```
        """
        return self._inner.is_generic()

    @classmethod
    def _from_inner(cls, inner: PyNoArchKind) -> "NoArchKind":
        """Create a NoArchKind from a PyNoArchKind."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    def __str__(self) -> str:
        """
        Get the string representation of the NoArch kind.

        Returns
        -------
        str
            The string representation of the NoArch kind.
        """
        return str(self._inner)


class ValueString:
    """A string value wrapper."""

    _inner: PyValueString

    def __init__(self, value: str):
        self._inner = PyValueString(value)

    @classmethod
    def concrete(cls, value: str) -> "ValueString":
        """
        Create a concrete string value.

        Examples
        --------
        ```python
        >>> val = ValueString.concrete("hello")
        >>> val.is_concrete()
        True
        >>> val.get_concrete()
        'hello'
        >>>
        ```
        """
        instance = cls.__new__(cls)
        instance._inner = PyValueString.concrete(value)
        return instance

    @classmethod
    def template(cls, template: str) -> "ValueString":
        """
        Create a template string value.

        Examples
        --------
        ```python
        >>> val = ValueString.template("{{ version }}")
        >>> val.is_template()
        True
        >>> val.get_template()
        '{{ version }}'
        >>>
        ```
        """
        instance = cls.__new__(cls)
        instance._inner = PyValueString.template(template)
        return instance

    def is_concrete(self) -> bool:
        """
        Check if this is a concrete value.

        Examples
        --------
        ```python
        >>> ValueString.concrete("test").is_concrete()
        True
        >>> ValueString.template("{{ var }}").is_concrete()
        False
        >>>
        ```
        """
        return self._inner.is_concrete()

    def is_template(self) -> bool:
        """
        Check if this is a template value.

        Examples
        --------
        ```python
        >>> ValueString.template("{{ var }}").is_template()
        True
        >>> ValueString.concrete("test").is_template()
        False
        >>>
        ```
        """
        return self._inner.is_template()

    def get_concrete(self) -> Optional[str]:
        """Get the concrete value."""
        return self._inner.get_concrete()

    def get_template(self) -> Optional[str]:
        """Get the template value."""
        return self._inner.get_template()

    @classmethod
    def _from_inner(cls, inner: PyValueString) -> "ValueString":
        """Create a ValueString from a PyValueString."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    def __str__(self) -> str:
        """
        Get the string representation of the ValueString.

        Returns
        -------
        str
            The concrete value if available, otherwise the template.
        """
        return str(self._inner)


class ValueU64:
    """A U64 value wrapper."""

    _inner: PyValueU64

    @classmethod
    def concrete(cls, value: int) -> "ValueU64":
        """
        Create a concrete U64 value.

        Examples
        --------
        ```python
        >>> val = ValueU64.concrete(42)
        >>> val.is_concrete()
        True
        >>> val.get_concrete()
        42
        >>>
        ```
        """
        instance = cls.__new__(cls)
        instance._inner = PyValueU64.concrete(value)
        return instance

    @classmethod
    def template(cls, template: str) -> "ValueU64":
        """
        Create a template U64 value.

        Examples
        --------
        ```python
        >>> val = ValueU64.template("{{ build_number }}")
        >>> val.is_template()
        True
        >>> val.get_template()
        '{{ build_number }}'
        >>>
        ```
        """
        instance = cls.__new__(cls)
        instance._inner = PyValueU64.template(template)
        return instance

    def is_concrete(self) -> bool:
        """
        Check if this is a concrete value.

        Examples
        --------
        ```python
        >>> ValueU64.concrete(123).is_concrete()
        True
        >>> ValueU64.template("{{ num }}").is_concrete()
        False
        >>>
        ```
        """
        return self._inner.is_concrete()

    def is_template(self) -> bool:
        """
        Check if this is a template value.

        Examples
        --------
        ```python
        >>> ValueU64.template("{{ num }}").is_template()
        True
        >>> ValueU64.concrete(123).is_template()
        False
        >>>
        ```
        """
        return self._inner.is_template()

    def get_concrete(self) -> Optional[int]:
        """Get the concrete value."""
        return self._inner.get_concrete()

    def get_template(self) -> Optional[str]:
        """Get the template value."""
        return self._inner.get_template()

    @classmethod
    def _from_inner(cls, inner: PyValueU64) -> "ValueU64":
        """Create a ValueU64 from a PyValueU64."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance


class ConditionalRequirements:
    """A conditional requirements wrapper."""

    _inner: PyConditionalRequirements

    def __init__(self) -> None:
        self._inner = PyConditionalRequirements()

    @property
    def build(self) -> "VecItemPackageDependency":
        """Get the build requirements."""
        return VecItemPackageDependency._from_inner(self._inner.build)

    @build.setter
    def build(self, value: Union[List[ItemPackageDependency], "VecItemPackageDependency"]) -> None:
        """Set the build requirements."""
        if isinstance(value, VecItemPackageDependency):
            self._inner.build = value._inner

        else:
            vec = VecItemPackageDependency()
            vec.extend(value)
            self._inner.build = vec._inner

    @property
    def host(self) -> "VecItemPackageDependency":
        """Get the host requirements."""
        return VecItemPackageDependency._from_inner(self._inner.host)

    @host.setter
    def host(self, value: Union[List[ItemPackageDependency], "VecItemPackageDependency"]) -> None:
        """Set the host requirements."""
        if isinstance(value, VecItemPackageDependency):
            self._inner.host = value._inner
        else:
            vec = VecItemPackageDependency()
            vec.extend(value)
            self._inner.host = vec._inner

    @property
    def run(self) -> "VecItemPackageDependency":
        """Get the run requirements."""
        return VecItemPackageDependency._from_inner(self._inner.run)

    @run.setter
    def run(self, value: Union[List[ItemPackageDependency], "VecItemPackageDependency"]) -> None:
        """Set the run requirements."""
        if isinstance(value, VecItemPackageDependency):
            self._inner.run = value._inner
        else:
            vec = VecItemPackageDependency()
            vec.extend(value)
            self._inner.run = vec._inner

    @property
    def run_constraints(self) -> "VecItemPackageDependency":
        """Get the run constraints."""
        return VecItemPackageDependency._from_inner(self._inner.run_constraints)

    @run_constraints.setter
    def run_constraints(self, value: Union[List[ItemPackageDependency], "VecItemPackageDependency"]) -> None:
        """Set the run constraints."""
        if isinstance(value, VecItemPackageDependency):
            self._inner.run_constraints = value._inner
        else:
            vec = VecItemPackageDependency()
            vec.extend(value)
            self._inner.run_constraints = vec._inner

    def resolve(self, host_platform: Optional[Platform] = None) -> "PackageSpecDependencies":
        """Resolve the requirements."""
        py_platform = host_platform._inner if host_platform else None
        return PackageSpecDependencies._from_inner(self._inner.resolve(py_platform))

    @classmethod
    def _from_inner(cls, inner: PyConditionalRequirements) -> "ConditionalRequirements":
        """Create a ConditionalRequirements from a PyConditionalRequirements."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    def __str__(self) -> str:
        """
        Get the string representation of the ConditionalRequirements.

        Returns
        -------
        str
            The string representation of the build, host, run, and run constraints.
        """
        return str(self._inner)


class About:
    """An about information wrapper."""

    _inner: PyAbout

    def __init__(self) -> None:
        self._inner = PyAbout()

    @property
    def homepage(self) -> Optional[str]:
        """Get the homepage."""
        return self._inner.homepage

    @property
    def license(self) -> Optional[str]:
        """Get the license."""
        return self._inner.license

    @property
    def summary(self) -> Optional[str]:
        """Get the summary."""
        return self._inner.summary

    @property
    def description(self) -> Optional[str]:
        """Get the description."""
        return self._inner.description

    @classmethod
    def _from_inner(cls, inner: PyAbout) -> "About":
        """Create an About from a PyAbout."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance


class Extra:
    """An extra information wrapper."""

    _inner: PyExtra

    def __init__(self) -> None:
        self._inner = PyExtra()

    @classmethod
    def _from_inner(cls, inner: PyExtra) -> "Extra":
        """Create an Extra from a PyExtra."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def recipe_maintainers(self) -> "ConditionalListString":
        """Get the recipe maintainers."""
        return self._inner.recipe_maintainers


class PackageSpecDependencies:
    """A package spec dependencies wrapper."""

    _inner: PyPackageSpecDependencies

    @classmethod
    def _from_inner(cls, inner: PyPackageSpecDependencies) -> "PackageSpecDependencies":
        """Create a PackageSpecDependencies from a PyPackageSpecDependencies."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def host(self) -> Dict[str, str]:
        """Get the host dependencies."""
        return self._inner.host

    @property
    def run(self) -> Dict[str, str]:
        """Get the run dependencies."""
        return self._inner.run

    @property
    def run_constraints(self) -> Dict[str, str]:
        """Get the run constraints."""
        return self._inner.run_constraints

    @property
    def build(self) -> Dict[str, str]:
        """Get the build dependencies."""
        return self._inner.build


class ItemString:
    """A package dependency item wrapper."""

    _inner: PyItemString

    @classmethod
    def _from_inner(cls, inner: PyItemString) -> "ItemString":
        """Create an ItemString from a FFI PyItemString."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance


class Source:
    """A source wrapper."""

    _inner: PySource

    @classmethod
    def url(cls, url: str) -> "Source":
        """Create a URL source."""
        instance = cls.__new__(cls)
        instance._inner = PySource.url(PyUrlSource(url, None))
        return instance

    @classmethod
    def path(cls, path: Path) -> "Source":
        """Create a path source."""
        instance = cls.__new__(cls)
        instance._inner = PySource.path(PyPathSource(str(path), None))
        return instance

    @classmethod
    def _from_inner(cls, inner: PySource) -> "Source":
        """Create a Source from a PySource."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance


class UrlSource:
    """A URL source wrapper."""

    _inner: PyUrlSource

    def __init__(self, url: str, sha: Optional[str] = None):
        self._inner = PyUrlSource(url, sha)

    @classmethod
    def _from_inner(cls, inner: PyUrlSource) -> "UrlSource":
        """Create a UrlSource from a PyUrlSource."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def url(self) -> str:
        """Get the URL."""
        return self._inner.url

    @property
    def sha(self) -> Optional[str]:
        """Get the SHA."""
        return self._inner.sha


class PathSource:
    """A path source wrapper."""

    _inner: PyPathSource

    def __init__(self, path: Path, sha: Optional[str] = None):
        self._inner = PyPathSource(path, sha)

    @classmethod
    def _from_inner(cls, inner: PyPathSource) -> "PathSource":
        """Create a PathSource from a PyPathSource."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def path(self) -> str:
        """Get the path."""
        return self._inner.path
