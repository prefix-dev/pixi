# pyright: reportUnknownMemberType=false, reportUnknownVariableType=false, reportMissingParameterType=false, reportUnknownParameterType=false, reportUnknownArgumentType=false

import glob
import json
import tomllib

from pathlib import Path
from typing import Any

import pytest
import jsonschema

HERE = Path(__file__).parent
EXAMPLES = HERE / "examples"
DOC_EXAMPLES = HERE.joinpath("..", "docs", "source_files", "pixi_tomls")
VALID = {ex.stem: ex for ex in (EXAMPLES / "valid").glob("*.toml")} | {
    ex.stem: ex for ex in DOC_EXAMPLES.glob("*.toml")
}
INVALID = {ex.stem: ex for ex in (EXAMPLES / "invalid").glob("*.toml")}


@pytest.fixture(scope="module", params=VALID)
def valid_manifest(request) -> dict[str, Any]:
    manifest = VALID[request.param].read_text()
    manifest_toml = tomllib.loads(manifest)
    return manifest_toml


@pytest.fixture(scope="module", params=INVALID)
def invalid_manifest(request) -> dict[str, Any]:
    manifest = INVALID[request.param].read_text()
    manifest_toml = tomllib.loads(manifest)
    return manifest_toml


def _real_manifest_path():
    # Get all `pixi.toml` files from the project
    for manifest in glob.glob("../**/**/pixi.toml"):
        if "invalid" in manifest:
            continue
        yield manifest


@pytest.fixture(params=_real_manifest_path())
def real_manifest_path(request):
    return request.param


@pytest.fixture(scope="session")
def manifest_schema():
    with open("schema.json") as f:
        schema = json.load(f)
    return schema


@pytest.fixture(scope="session")
def validator(manifest_schema):
    validator_cls = jsonschema.validators.validator_for(manifest_schema)  # pyright: ignore[reportAttributeAccessIssue]
    return validator_cls(manifest_schema)


def test_manifest_schema_valid(validator, valid_manifest):
    validator.validate(valid_manifest)


def test_manifest_schema_invalid(validator, invalid_manifest):
    with pytest.raises(jsonschema.ValidationError):
        validator.validate(invalid_manifest)


def test_real_manifests(real_manifest_path, validator):
    print(real_manifest_path)
    with open(real_manifest_path) as f:
        manifest = f.read()
    manifest_toml = tomllib.loads(manifest)
    validator.validate(manifest_toml)
