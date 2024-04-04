echo "Running test_examples.sh"

echo "Running the polarify example:"
pixi run -v --manifest-path examples/polarify/pixi.toml test
pixi run -v --manifest-path examples/polarify/pixi.toml -e pl020 test

echo "Running the pypi example:"
pixi run -v --manifest-path examples/pypi/pixi.toml test

echo "Running the conda_mapping example:"
pixi run -v --manifest-path examples/conda_mapping/pixi.toml test

echo "Running the solve-groups example:"
pixi run -v --manifest-path examples/solve-groups/pixi.toml -e min-py38 test
pixi run -v --manifest-path examples/solve-groups/pixi.toml -e max-py310 test
