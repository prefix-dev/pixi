import pytest
from pathlib import Path
import pathlib
import os
import subprocess
import tempfile

StrPath = str | os.PathLike[str]


def run(args: list[StrPath], cwd: StrPath) -> None:
    proc: subprocess.CompletedProcess[bytes] = subprocess.run(
        args, cwd=cwd, capture_output=True, check=False
    )
    proc.check_returncode()


def test_wheel(pixi_path: str | None, wheel_name: str):
    """
    Create a temporary directory and install the wheel in it
    """

    with tempfile.TemporaryDirectory() as _dtemp:
        dtemp: pathlib.Path = pathlib.Path(_dtemp)
        run(["pixi", "init"], cwd=dtemp)
        run(["pixi", "add", "python==3.12.*"], cwd=dtemp)
        run(
            ["pixi", "-vvv", "add", "--pypi", wheel_name],
            cwd=dtemp,
        )


def read_wheel_file():
    """
    Read the wheel file `wheels.txt` and return the name of the wheel
    which is split per line
    """
    wheel_path = Path(__file__).parent / Path("wheels.txt")
    with wheel_path.open("r") as f:
        return f.read().splitlines()


def pytest_generate_tests(metafunc):
    if "wheel_name" in metafunc.fixturenames:
        metafunc.parametrize("wheel_name", read_wheel_file())


@pytest.fixture(scope="session")
def pixi_path(pytestconfig):
    return pytestconfig.getoption("pixi_exec")
