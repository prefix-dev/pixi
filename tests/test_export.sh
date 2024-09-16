# Run from the root of the project using `bash tests/test_export.sh`
set -e
echo "Running test_export.sh"

echo "Exporting the export test environment"
pixi project export conda-environment --manifest-path src/cli/project/export/test-data/testenv/pixi.toml | tee test-env.yml
echo "Creating the export test environment with micromamba"
micromamba create -y -f test-env.yml -n export-test
micromamba env list
micromamba env remove -y -n export-test

echo "Exporting an environment.yml with pip extras"
pixi project export conda-environment --manifest-path examples/pypi/pixi.toml | tee test-env-pip-extras.yml
echo "Creating the pip extra test environment with micromamba"
micromamba create -y -f test-env-pip-extras.yml -n export-test-pip-extras
micromamba env list
micromamba env remove -y -n export-test-pip-extras

echo "Export an environment.yml with editable source dependencies"
pixi project export conda-environment --manifest-path examples/pypi-source-deps/pixi.toml | tee test-env-source-deps.yml
echo "Creating the editable source dependencies test environment with micromamba"
micromamba create -y -f test-env-source-deps.yml -n export-test-source-deps
micromamba env list
micromamba env remove -y -n export-test-source-deps

echo "Export an environment.yml with custom pip registry"
pixi project export conda-environment --manifest-path examples/pypi-custom-registry/pixi.toml | tee test-env-custom-registry.yml
echo "Creating the custom pip registry test environment with micromamba"
micromamba create -y -f test-env-custom-registry.yml -n export-test-custom-registry
micromamba env list
micromamba env remove -y -n export-test-custom-registry

echo "Export an environment.yml with pip find links"
pixi project export conda-environment --manifest-path examples/pypi-find-links/pixi.toml | tee test-env-find-links.yml
echo "Creating the pip find links test environment with micromamba"
micromamba create -y -f test-env-find-links.yml -n export-test-find-links
micromamba env list
micromamba env remove -y -n export-test-find-links

echo "Export an environment.yml from a pyproject.toml that has caused panics"
pixi project export conda-environment --manifest-path examples/docker/pyproject.toml | tee test-env-pyproject-panic.yml
echo "Creating the pyproject.toml panic test environment with micromamba"
micromamba create -y -f test-env-pyproject-panic.yml -n export-test-pyproject-panic
micromamba env list
micromamba env remove -y -n export-test-pyproject-panic
