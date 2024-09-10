from typing_extensions import Iterable
import pytest
from pathlib import Path
import pathlib
import os
import subprocess
import tempfile
from read_wheels import Package, WheelTest

StrPath = str | os.PathLike[str]


def run(args: list[StrPath], cwd: StrPath | None = None) -> None:
    proc: subprocess.CompletedProcess[bytes] = subprocess.run(
        args, cwd=cwd, capture_output=True, check=False
    )
    proc.check_returncode()


def test_wheel(pixi_path: str | None, wheel: Package):
    """
    Create a temporary directory and install the wheel in it
    """
    with tempfile.TemporaryDirectory() as _dtemp:
        dtemp: pathlib.Path = pathlib.Path(_dtemp)
        manifest_path = dtemp / "pixi.toml"
        run(["pixi", "init"], cwd=dtemp)
        run(["pixi", "add", "--manifest-path", manifest_path, "python==3.12.*"])
        print(wheel.to_add_cmd())
        run(
            ["pixi", "-vvv", "add", "--manifest-path", manifest_path, "--pypi", wheel.to_add_cmd()],
        )


def read_wheel_file() -> Iterable[Package]:
    """
    Read the wheel file `wheels.txt` and return the name of the wheel
    which is split per line
    """
    wheel_path = Path(__file__).parent / Path("wheels.toml")
    return WheelTest.from_toml(wheel_path).to_packages()


def pytest_generate_tests(metafunc):
    if "wheel" in metafunc.fixturenames:
        metafunc.parametrize("wheel", read_wheel_file())


@pytest.fixture(scope="session")
def pixi_path(pytestconfig):
    return pytestconfig.getoption("pixi_exec")
