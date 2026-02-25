from __future__ import annotations

import glob
import json
import tomllib

from pathlib import Path
from typing import Any, TYPE_CHECKING

import pytest
from jsonschema.validators import Draft7Validator
from jsonschema import ValidationError
from jsonschema.protocols import Validator

if TYPE_CHECKING:
    from collections.abc import Iterator

TRawSchemata = dict[str, dict[str, dict[str, Any]]]
TValidators = dict[str, dict[str, Validator]]

HERE = Path(__file__).parent
EXAMPLES = HERE / "examples"
DOC_EXAMPLES = HERE.joinpath("..", "docs", "source_files", "pixi_tomls")
VALID = {ex.stem: ex for ex in (EXAMPLES / "valid").glob("*.toml")} | {
    ex.stem: ex for ex in DOC_EXAMPLES.glob("*.toml")
}
INVALID = {ex.stem: ex for ex in (EXAMPLES / "invalid").glob("*.toml")}

PIXI_TOML = "pixi.toml"
PYPROJECT_TOML = "pyproject.toml"
PIXI_SCHEMA = "schema.json"
PYPROJECT_SCHEMA = "pyproject.schema.json"
PYPROJECT_PARTIAL_SCHEMA = "pyproject.partial-pixi.json"

SCHEMAS_FOR_FILE: dict[str, dict[str, list[str]]] = {
    PIXI_TOML: {PIXI_SCHEMA: []},
    PYPROJECT_TOML: {
        PYPROJECT_PARTIAL_SCHEMA: ["tool", "pixi"],
        PYPROJECT_SCHEMA: [],
    },
}
SKIP_REAL_MANIFEST = {(PYPROJECT_PARTIAL_SCHEMA, "py-pixi-build-backend"): "no pixi tool"}


@pytest.fixture(scope="module", params=VALID)
def valid_manifest(request: pytest.FixtureRequest) -> dict[str, Any]:
    manifest = VALID[request.param].read_text()
    manifest_toml = tomllib.loads(manifest)
    return manifest_toml


@pytest.fixture(scope="module", params=INVALID)
def invalid_manifest(request: pytest.FixtureRequest) -> dict[str, Any]:
    manifest = INVALID[request.param].read_text()
    manifest_toml = tomllib.loads(manifest)
    return manifest_toml


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
            validator = Draft7Validator({**schema}, format_checker=Draft7Validator.FORMAT_CHECKER)
            validators.setdefault(manifest_name, {}).update({schema_name: validator})
    return validators


@pytest.fixture(scope="session")
def validator(all_validators: TValidators) -> Validator:
    return all_validators[PIXI_TOML][PIXI_SCHEMA]


def test_manifest_schema_valid(validator: Validator, valid_manifest: dict[str, Any]) -> None:
    validator.validate(valid_manifest)


def test_manifest_schema_invalid(validator: Validator, invalid_manifest: dict[str, Any]) -> None:
    with pytest.raises(ValidationError):
        validator.validate(invalid_manifest)


def test_real_manifests(
    real_manifest: tuple[Path, dict[str, Any]], all_validators: TValidators
) -> None:
    path, manifest = real_manifest
    print(path)

    for schema_path, validator in all_validators[path.name].items():
        print("\t", schema_path, end="... ")
        skip_reason = SKIP_REAL_MANIFEST.get((schema_path, path.parent.name))
        if skip_reason:
            print(skip_reason)
            continue
        subpath = SCHEMAS_FOR_FILE[path.name][schema_path]
        partial: dict[str, Any] | None = manifest
        for segment in subpath:
            partial = partial[segment] if partial and segment in partial else None
        if subpath and not partial:
            print(f"no {subpath}")
        errors = [m.message for m in validator.iter_errors(partial)]
        err_count = len(errors)
        print(err_count, "errors")
        assert not errors
