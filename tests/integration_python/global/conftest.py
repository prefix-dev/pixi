import pytest
from pathlib import Path


@pytest.fixture
def trampoline_channel_1(channels: Path) -> str:
    return channels.joinpath("trampoline_1").as_uri()


@pytest.fixture
def trampoline_channel_2(channels: Path) -> str:
    return channels.joinpath("trampoline_2").as_uri()
