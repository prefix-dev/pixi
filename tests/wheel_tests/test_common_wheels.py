import pytest
import pathlib
import subprocess
import tempfile
from read_wheels import Package, read_wheel_file
import time
from record_results import record_result
from helpers import add_system_requirements, log_called_process_error, run


def test_wheel(pixi_path: str | None, package: Package, testrun_uid: str):
    """
    Create a temporary directory and install the wheel in it.
    The `testrun_uid` is a unique identifier for the test run
    this is created by pytest-xdist
    """
    start = time.perf_counter()
    try:
        with tempfile.TemporaryDirectory() as _dtemp:
            dtemp: pathlib.Path = pathlib.Path(_dtemp)
            manifest_path = dtemp / "pixi.toml"
            run(["pixi", "init"], cwd=dtemp)

            # Check if we need to add system-requirements
            # There is no CLI for it currently so we need to manually edit the file
            if package.spec.system_requirements:
                add_system_requirements(manifest_path, package.spec.system_requirements)

            run(
                ["pixi", "add", "--no-progress", "--manifest-path", manifest_path, "python==3.12.*"]
            )

            run_args = [
                "pixi",
                "-vvv",
                "add",
                "--no-progress",
                "--manifest-path",
                manifest_path,
                "--pypi",
                package.to_add_cmd(),
            ]

            # Add for another platform, if specified
            if package.spec.target:
                run_args.extend(["--platform", package.spec.target])

            run(run_args)
            # Record the success of the test
            record_result(
                testrun_uid, package.to_add_cmd(), "passed", time.perf_counter() - start, ""
            )
    except subprocess.CalledProcessError as e:
        # Record the failure details
        record_result(
            testrun_uid, package.to_add_cmd(), "failed", time.perf_counter() - start, str(e)
        )
        log_called_process_error(package.to_add_cmd(), e, std_err_only=True)


def pytest_generate_tests(metafunc):
    """
    This generates the test for the wheels by reading the wheels from the toml specification
    creates a test for each entry in the toml file
    """
    if "package" in metafunc.fixturenames:
        packages = read_wheel_file()
        metafunc.parametrize("package", [pytest.param(w, id=f"{w.to_add_cmd()}") for w in packages])


@pytest.fixture(scope="session")
def pixi_path(pytestconfig):
    return pytestconfig.getoption("pixi_exec")
