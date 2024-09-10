from typing import Iterable
from multiprocessing import Lock
import pytest
from pathlib import Path
import pathlib
import os
import subprocess
import tempfile
from read_wheels import Package, WheelTest
import json
import time

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
    start = time.perf_counter()
    try:
        with tempfile.TemporaryDirectory() as _dtemp:
            dtemp: pathlib.Path = pathlib.Path(_dtemp)
            manifest_path = dtemp / "pixi.toml"
            run(["pixi", "init"], cwd=dtemp)
            run(["pixi", "add", "--manifest-path", manifest_path, "python==3.12.*"])
            run(
                [
                    "pixi",
                    "-vvv",
                    "add",
                    "--manifest-path",
                    manifest_path,
                    "--pypi",
                    wheel.to_add_cmd(),
                ],
            )
            # Record the success of the test
            record_result(wheel.name, "passed", time.perf_counter() - start, "")
    except subprocess.CalledProcessError as e:
        # Record the failure details
        record_result(wheel.name, "failed", time.perf_counter() - start, str(e))


def read_wheel_file() -> Iterable[Package]:
    """
    Read the wheel file `wheels.txt` and return the name of the wheel
    which is split per line
    """
    wheel_path = Path(__file__).parent / Path("wheels.toml")
    return WheelTest.from_toml(wheel_path).to_packages()


def pytest_generate_tests(metafunc):
    """
    This generates the test for the wheels
    """
    if "wheel" in metafunc.fixturenames:
        wheels = read_wheel_file()
        metafunc.parametrize("wheel", [pytest.param(w, id=f"{w.name}") for w in wheels])


@pytest.fixture(scope="session")
def pixi_path(pytestconfig):
    return pytestconfig.getoption("pixi_exec")


lock = Lock()
RESULTS_FILE = "test_results.json"


def record_result(name, outcome, duration, details):
    """
    Collects test status after each test run, compatible with pytest-xdist.
    """
    result = {"name": name, "outcome": outcome, "duration": duration, "longrepr": details}

    # Lock for thread-safe write access to the results file
    with lock:
        path = Path(RESULTS_FILE)
        results = []
        if path.exists():
            with open(RESULTS_FILE, "r") as f:
                results = json.load(f)
        results.append(result)
        with open(RESULTS_FILE, "w") as f:
            json.dump(results, f)
