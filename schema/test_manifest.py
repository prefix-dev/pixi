import glob
import json
import tomllib

import pytest
from jsonschema import validate
from jsonschema.exceptions import ValidationError


@pytest.fixture(
    scope="module",
    params=[
        "minimal",
        "full",
    ],
)
def valid_manifest(request) -> str:
    manifest_name = request.param
    with open(f"examples/valid/{manifest_name}.toml") as f:
        manifest = f.read()
    manifest_toml = tomllib.loads(manifest)
    return manifest_toml


@pytest.fixture(
    scope="module",
    params=[
        "empty",
        "no_channel",
    ],
)
def invalid_manifest(request) -> str:
    manifest_name = request.param
    with open(f"examples/invalid/{manifest_name}.toml") as f:
        manifest = f.read()
    manifest_toml = tomllib.loads(manifest)
    return manifest_toml


# @pytest.fixture()
def _real_manifest_path():
    # Get all `pixi.toml` files from the project
    for manifest in glob.glob("../**/**/pixi.toml"):
        if "invalid" in manifest:
            continue
        yield manifest
    #     manifest_paths += [manifest]
    # return manifest_paths


@pytest.fixture(params=_real_manifest_path())
def real_manifest_path(request):
    return request.param


@pytest.fixture()
def manifest_schema():
    with open("schema.json") as f:
        schema = json.load(f)
    return schema


def test_manifest_schema_valid(manifest_schema, valid_manifest):
    validate(instance=valid_manifest, schema=manifest_schema)


def test_manifest_schema_invalid(manifest_schema, invalid_manifest):
    with pytest.raises(ValidationError):
        validate(instance=invalid_manifest, schema=manifest_schema)


def test_real_manifests(real_manifest_path, manifest_schema):
    print(real_manifest_path)
    with open(real_manifest_path) as f:
        manifest = f.read()
    manifest_toml = tomllib.loads(manifest)
    validate(instance=manifest_toml, schema=manifest_schema)
