#!/usr/bin/env bash

# Run from the root of the project using `bash tests/scripts/test_export.sh`
set -e
set -x
echo "Running test_export.sh"

echo "Activating 'export-test' env"
eval "$(pixi shell-hook)"
unset PIXI_IN_SHELL
echo "Exporting the export test environment"
cd tests/data/mock-projects/test-project-export
pixi project export conda-environment | tee test-env.yml
echo "Creating the export test environment with micromamba"
micromamba create -y -f test-env.yml -n export-test
micromamba env list
micromamba env remove -y -n export-test
# Test for correct subdirectory format
export _PIXITEST_TMP=$(mktemp -d)
pixi init -i test-env.yml $_PIXITEST_TMP
pixi install --manifest-path $_PIXITEST_TMP
rm test-env.yml
cd ../../../..

# Setuptools error with env_test_package
# echo "Exporting an environment.yml with pip extras"
# cd examples/pypi
# pixi project export conda-environment | tee test-env-pip-extras.yml
# echo "Creating the pip extra test environment with micromamba"
# micromamba create -y -f test-env-pip-extras.yml -n export-test-pip-extras
# micromamba env list
# micromamba env remove -y -n export-test-pip-extras
# rm test-env-pip-extras.yml
# cd ../..

echo "Export an environment.yml with editable source dependencies"
cd examples/pypi-source-deps
pixi project export conda-environment | tee test-env-source-deps.yml
echo "Creating the editable source dependencies test environment with micromamba"
micromamba create -y -f test-env-source-deps.yml -n export-test-source-deps
micromamba env list
micromamba env remove -y -n export-test-source-deps
rm test-env-source-deps.yml
cd ../..

echo "Export an environment.yml with custom pip registry"
cd examples/pypi-custom-registry
pixi project export conda-environment | tee test-env-custom-registry.yml
echo "Creating the custom pip registry test environment with micromamba"
micromamba create -y -f test-env-custom-registry.yml -n export-test-custom-registry
micromamba env list
micromamba env remove -y -n export-test-custom-registry
rm test-env-custom-registry.yml
cd ../..

echo "Export an environment.yml with pip find links"
cd examples/pypi-find-links
pixi project export conda-environment | tee test-env-find-links.yml
echo "Creating the pip find links test environment with micromamba"
micromamba create -y -f test-env-find-links.yml -n export-test-find-links
micromamba env list
micromamba env remove -y -n export-test-find-links
rm test-env-find-links.yml
cd ../..

echo "Export an environment.yml from a pyproject.toml that has caused panics"
cd examples/docker
pixi project export conda-environment | tee test-env-pyproject-panic.yml
echo "Creating the pyproject.toml panic test environment with micromamba"
micromamba create -y -f test-env-pyproject-panic.yml -n export-test-pyproject-panic
micromamba env list
micromamba env remove -y -n export-test-pyproject-panic
rm test-env-pyproject-panic.yml
cd ../..
