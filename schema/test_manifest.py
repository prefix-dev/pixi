from __future__ import annotations

import glob
import json
import tomllib
import pprint

from pathlib import Path
from typing import Any, TYPE_CHECKING

import pytest
from jsonschema_rs import validator_for, Validator, ValidationError

if TYPE_CHECKING:
    from collections.abc import Iterator

TRawSchemata = dict[str, dict[str, dict[str, Any]]]
TValidators = dict[str, dict[str, Validator]]

HERE = Path(__file__).parent
EXAMPLES = HERE / "examples"
EXAMPLES_PY = HERE / "pyproject_toml/examples"
DOC_EXAMPLES = HERE.joinpath("..", "docs", "source_files", "pixi_tomls")
VALID = {ex.stem: ex for ex in (EXAMPLES / "valid").glob("*.toml")} | {
    ex.stem: ex for ex in DOC_EXAMPLES.glob("*.toml")
}
INVALID = {ex.stem: ex for ex in (EXAMPLES / "invalid").glob("*.toml")}
VALID_PY = {ex.stem: ex for ex in (EXAMPLES_PY / "valid").glob("*.toml")}
INVALID_PY = {ex.stem: ex for ex in (EXAMPLES_PY / "invalid").glob("*.toml")}

PIXI_TOML = "pixi.toml"
PYPROJECT_TOML = "pyproject.toml"
PIXI_SCHEMA = "schema.json"
PYPROJECT_SCHEMA = "pyproject_toml/schema.json"
PYPROJECT_PARTIAL_SCHEMA = "pyproject_toml/partial-pixi.json"

SCHEMAS_FOR_FILE: dict[str, dict[str, list[str]]] = {
    PIXI_TOML: {PIXI_SCHEMA: []},
    PYPROJECT_TOML: {
        PYPROJECT_PARTIAL_SCHEMA: ["tool", "pixi"],
        PYPROJECT_SCHEMA: [],
    },
}
SKIP_REAL_MANIFEST = {(PYPROJECT_PARTIAL_SCHEMA, "py-pixi-build-backend"): "no pixi tool"}


def _from_request(request: pytest.FixtureRequest, fixture_set: dict[str, Path]) -> dict[str, Any]:
    manifest = fixture_set[request.param].read_text()
    manifest_toml = tomllib.loads(manifest)
    return manifest_toml


@pytest.fixture(scope="module", params=VALID)
def valid_manifest(request: pytest.FixtureRequest) -> dict[str, Any]:
    return _from_request(request, VALID)


@pytest.fixture(scope="module", params=INVALID)
def invalid_manifest(request: pytest.FixtureRequest) -> dict[str, Any]:
    return _from_request(request, INVALID)


@pytest.fixture(scope="module", params=VALID_PY)
def valid_pyproject(request: pytest.FixtureRequest) -> dict[str, Any]:
    return _from_request(request, VALID_PY)


@pytest.fixture(scope="module", params=INVALID_PY)
def invalid_pyproject(request: pytest.FixtureRequest) -> dict[str, Any]:
    return _from_request(request, INVALID_PY)


def _real_path(*patterns: str) -> Iterator[str]:
    """Get all files from the project."""
    for pattern in patterns:
        for manifest in glob.glob(pattern):
            if any(e in manifest for e in ["invalid"]):
                continue
            yield manifest


@pytest.fixture(params=_real_path("../**/**/pixi.toml", "../**/**/pyproject.toml"))
def real_manifest(request: pytest.FixtureRequest) -> tuple[Path, dict[str, Any]]:
    path = Path(f"{request.param}")
    return path, tomllib.loads(path.read_text(encoding="utf-8"))


@pytest.fixture(scope="session")
def manifest_schemata() -> TRawSchemata:
    schemata: TRawSchemata = {}
    for manifest_name, schema_names in SCHEMAS_FOR_FILE.items():
        for schema_name in schema_names:
            with open(schema_name) as f:
                schema = json.load(f)
                schemata.setdefault(manifest_name, {}).update({schema_name: schema})
    return schemata


@pytest.fixture(scope="session")
def all_validators(manifest_schemata: TRawSchemata) -> TValidators:
    validators: TValidators = {}
    for manifest_name, schemata in manifest_schemata.items():
        for schema_name, schema in schemata.items():
            validator = validator_for(schema, validate_formats=True, ignore_unknown_formats=False)
            validators.setdefault(manifest_name, {}).update({schema_name: validator})
    return validators


@pytest.fixture(scope="session")
def validator(all_validators: TValidators) -> Validator:
    return all_validators[PIXI_TOML][PIXI_SCHEMA]


@pytest.fixture(scope="session")
def pyproject_validator(all_validators: TValidators) -> Validator:
    return all_validators[PYPROJECT_TOML][PYPROJECT_SCHEMA]


@pytest.fixture(scope="session")
def pyproject_partial_validator(all_validators: TValidators) -> Validator:
    return all_validators[PYPROJECT_TOML][PYPROJECT_PARTIAL_SCHEMA]


def test_manifest_schema_valid(validator: Validator, valid_manifest: dict[str, Any]) -> None:
    validator.validate(valid_manifest)


def test_manifest_schema_invalid(validator: Validator, invalid_manifest: dict[str, Any]) -> None:
    with pytest.raises(ValidationError):
        validator.validate(invalid_manifest)


def test_pyproject_schema_valid(
    pyproject_validator: Validator, valid_pyproject: dict[str, Any]
) -> None:
    pyproject_validator.validate(valid_pyproject)


def test_pyproject_schema_invalid(
    pyproject_validator: Validator, invalid_pyproject: dict[str, Any]
) -> None:
    with pytest.raises(ValidationError):
        pyproject_validator.validate(invalid_pyproject)


def _skip_no_tool(pyproject: dict[str, Any]) -> None:
    if "pixi" not in pyproject.get("tool", {}):
        pytest.skip("no [tool.pixi]")


def test_pyproject_partial_schema_valid(
    pyproject_partial_validator: Validator, valid_pyproject: dict[str, Any]
) -> None:
    _skip_no_tool(valid_pyproject)
    pyproject_partial_validator.validate(valid_pyproject["tool"]["pixi"])


def test_pyproject_partial_schema_invalid(
    pyproject_validator: Validator, invalid_pyproject: dict[str, Any]
) -> None:
    _skip_no_tool(invalid_pyproject)
    with pytest.raises(ValidationError):
        pyproject_validator.validate(invalid_pyproject)


def test_real_manifests(
    real_manifest: tuple[Path, dict[str, Any]], all_validators: TValidators
) -> None:
    path, manifest = real_manifest
    print(path)
    all_errors: dict[str, Any] = {}
    for schema_path, validator in all_validators[path.name].items():
        print("\t...", schema_path)
        skip_reason = SKIP_REAL_MANIFEST.get((schema_path, path.parent.name))
        if skip_reason:
            print("\t\t... skipped:", skip_reason)
            continue
        subpath = SCHEMAS_FOR_FILE[path.name][schema_path]
        partial = manifest
        for segment in subpath:
            partial = partial[segment]
        errors = validator.evaluate(partial).errors()
        all_errors[schema_path] = errors

    error_count = sum(map(len, all_errors.values()))
    if error_count:
        pprint.pprint(all_errors)
    assert not error_count
