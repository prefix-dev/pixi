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
def channels() -> Path:
    return Path(__file__).parent.parent.joinpath("data", "channels", "channels").resolve()


@pytest.fixture
def dummy_channel_1(channels: Path) -> str:
    return channels.joinpath("dummy_channel_1").as_uri()


@pytest.fixture
def dummy_channel_2(channels: Path) -> str:
    return channels.joinpath("dummy_channel_2").as_uri()


@pytest.fixture
def global_update_channel_1(channels: Path) -> str:
    return channels.joinpath("global_update_channel_1").as_uri()


@pytest.fixture
def non_self_expose_channel_1(channels: Path) -> str:
    return channels.joinpath("non_self_expose_channel_1").as_uri()


@pytest.fixture
def non_self_expose_channel_2(channels: Path) -> str:
    return channels.joinpath("non_self_expose_channel_2").as_uri()
