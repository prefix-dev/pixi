from pathlib import Path

import pytest


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--pixi-build",
        action="store",
        default="release",
        help="Specify the pixi build type (e.g., release or debug)",
    )


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
