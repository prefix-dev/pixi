#!/usr/bin/env bash

# Run from the root of the project using `bash tests/scripts/test-export.sh`
set -e
set -x
echo "Running test_export.sh"

echo "Exporting the export test environment"
pixi project export conda-environment --manifest-path tests/data/mock-projects/test-project-export --environment default | tee test-env.yml
echo "Creating the export test environment with micromamba"
micromamba create -y -f test-env.yml -n export-test --dry-run
# Test for correct subdirectory format
export _PIXITEST_TMP=$(mktemp -d)
pixi init -i test-env.yml $_PIXITEST_TMP
pixi lock --manifest-path $_PIXITEST_TMP
rm test-env.yml

echo "Export an environment.yml with editable source dependencies"
pixi project export conda-environment --manifest-path examples/pypi-source-deps --environment default | tee test-env-source-deps.yml
echo "Creating the editable source dependencies test environment with micromamba"
micromamba create -y -f test-env-source-deps.yml -n export-test-source-deps --dry-run
rm test-env-source-deps.yml

echo "Export an environment.yml with custom pip registry"
pixi project export conda-environment --manifest-path examples/pypi-custom-registry --environment default | tee test-env-custom-registry.yml
echo "Creating the custom pip registry test environment with micromamba"
micromamba create -y -f test-env-custom-registry.yml -n export-test-custom-registry --dry-run
rm test-env-custom-registry.yml

echo "Export an environment.yml with pip find links"
pixi project export conda-environment --manifest-path examples/pypi-find-links --environment default | tee test-env-find-links.yml
echo "Creating the pip find links test environment with micromamba"
micromamba create -y -f test-env-find-links.yml -n export-test-find-links --dry-run
rm test-env-find-links.yml

echo "Export an environment.yml from a pyproject.toml that has caused panics"
pixi project export conda-environment --manifest-path examples/docker --environment default | tee test-env-pyproject-panic.yml
echo "Creating the pyproject.toml panic test environment with micromamba"
micromamba create -y -f test-env-pyproject-panic.yml -n export-test-pyproject-panic --dry-run
rm test-env-pyproject-panic.yml
