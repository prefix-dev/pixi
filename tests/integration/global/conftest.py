import pytest

from pathlib import Path


@pytest.fixture
def trampoline_channel_1(test_data: Path) -> str:
    return test_data.joinpath("channels", "trampoline_1").as_uri()


@pytest.fixture
def trampoline_channel_2(test_data: Path) -> str:
    return test_data.joinpath("channels", "trampoline_2").as_uri()
