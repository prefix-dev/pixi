from pathlib import Path
import pytest


@pytest.fixture
def build_data(test_data: Path) -> Path:
    """
    Returns the pixi build test data
    """
    return test_data.joinpath("pixi_build")


@pytest.fixture
def examples_dir() -> Path:
    """
    Returns the path to the examples directory in the root of the repository
    """
    return Path(__file__).parents[3].joinpath("examples").resolve()
