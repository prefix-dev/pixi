from pathlib import Path

import pytest


@pytest.fixture
def pixi() -> Path:
    return Path(__file__).parent.joinpath("../../.pixi/target/release/pixi")


@pytest.fixture
def test_data() -> Path:
    return Path(__file__).parent.joinpath("test_data").resolve()


@pytest.fixture
def dummy_channel_1(test_data: Path) -> str:
    return test_data.joinpath("dummy_channel_1/output").as_uri()


@pytest.fixture
def dummy_channel_2(test_data: Path) -> str:
    return test_data.joinpath("dummy_channel_2/output").as_uri()


@pytest.fixture
def global_update_channel_1(test_data: Path) -> str:
    return test_data.joinpath("global_update_channel_1/output").as_uri()
