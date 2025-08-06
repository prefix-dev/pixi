from pathlib import Path

import pytest


@pytest.fixture
def trampoline_channel(channels: Path) -> str:
    return channels.joinpath("trampoline_channel").as_uri()


@pytest.fixture
def trampoline_path_channel(channels: Path) -> str:
    return channels.joinpath("trampoline_path_channel").as_uri()
