from pathlib import Path
from typing import Sequence

import pytest


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--pixi-build",
        action="store",
        default="release",
        help="Specify the pixi build type (e.g., release or debug)",
    )
    parser.addoption("--runslow", action="store_true", default=False, help="run slow tests")


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line("markers", "slow: mark test as slow to run")


def pytest_collection_modifyitems(config: pytest.Config, items: Sequence[pytest.Item]) -> None:
    if config.getoption("--runslow"):
        # --runslow given in cli: do not skip slow tests
        return
    skip_slow = pytest.mark.skip(reason="need --runslow option to run")
    for item in items:
        if "slow" in item.keywords:
            item.add_marker(skip_slow)


@pytest.fixture
def pixi(request: pytest.FixtureRequest) -> Path:
    pixi_build = request.config.getoption("--pixi-build")
    return Path(__file__).parent.joinpath(f"../../target-pixi/{pixi_build}/pixi")


@pytest.fixture
def test_data() -> Path:
    return Path(__file__).parent.joinpath("test_data").resolve()


@pytest.fixture
def dummy_channel_1(test_data: Path) -> str:
    return test_data.joinpath("channels", "dummy_channel_1").as_uri()


@pytest.fixture
def dummy_channel_2(test_data: Path) -> str:
    return test_data.joinpath("channels", "dummy_channel_2").as_uri()


@pytest.fixture
def global_update_channel_1(test_data: Path) -> str:
    return test_data.joinpath("channels", "global_update_channel_1").as_uri()


@pytest.fixture
def non_self_expose_channel_1(test_data: Path) -> str:
    return test_data.joinpath("channels", "non_self_expose_channel_1").as_uri()


@pytest.fixture
def non_self_expose_channel_2(test_data: Path) -> str:
    return test_data.joinpath("channels", "non_self_expose_channel_2").as_uri()
