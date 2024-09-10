from pathlib import Path

import pytest


@pytest.fixture
def pixi() -> Path:
    return Path(__file__).parent.joinpath("../../.pixi/target/release/pixi")
