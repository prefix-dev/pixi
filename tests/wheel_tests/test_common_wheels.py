import pytest
import os
import pathlib
import subprocess
from read_wheels import Package, read_wheel_file
import time
from record_results import record_result
from helpers import add_system_requirements, log_called_process_error, run
import sys


@pytest.mark.flaky(reruns=5, reruns_delay=1, condition=sys.platform.startswith("win32"))
def test_wheel(
    pixi: str, package: Package, testrun_uid: str, tmp_pixi_workspace: pathlib.Path
) -> None:
    """
    Create a temporary directory and install the wheel in it.
    The `testrun_uid` is a unique identifier for the test run
    this is created by pytest-xdist
    """
    start = time.perf_counter()
    try:
        # Path to the manifest file
        manifest_path = tmp_pixi_workspace / "pixi.toml"
        run([pixi, "init"], cwd=tmp_pixi_workspace)

        # Check if we need to add system-requirements
        # There is no CLI for it currently so we need to manually edit the file
        if package.spec.system_requirements:
            add_system_requirements(manifest_path, package.spec.system_requirements)

        # Add python to the project
        run([pixi, "add", "--no-progress", "--manifest-path", manifest_path, "python==3.12.*"])

        # Add the wheel to the project
        run_args: list[str | os.PathLike[str]] = [
            pixi,
            "-vvv",
            "add",
            "--no-progress",
            "--manifest-path",
            manifest_path,
            "--pypi",
            package.to_add_cmd(),
        ]

        # Add for another platform, if specified
        for platform in package.spec.target_iter():
            assert isinstance(platform, str)
            run_args.extend(["--platform", platform])

        run(run_args)
        # Record the success of the test
        record_result(testrun_uid, package.to_add_cmd(), "passed", time.perf_counter() - start, "")
    except subprocess.CalledProcessError as e:
        # Record the failure details
        record_result(
            testrun_uid, package.to_add_cmd(), "failed", time.perf_counter() - start, str(e)
        )
        # Log the error
        log_called_process_error(package.to_add_cmd(), e, std_err_only=True)
        # Re-raise the exception to fail the test
        raise e


def pytest_generate_tests(metafunc: pytest.Metafunc) -> None:
    """
    This generates the test for the wheels by reading the wheels from the toml specification
    creates a test for each entry in the toml file
    """
    if "package" in metafunc.fixturenames:
        packages = read_wheel_file()
        metafunc.parametrize("package", [pytest.param(w, id=f"{w.to_add_cmd()}") for w in packages])


@pytest.fixture(scope="session")
def pixi(pytestconfig: pytest.Config) -> pathlib.Path:
    # The command line argument overrides the default path
    if pytestconfig.getoption("pixi_exec"):
        return pathlib.Path(pytestconfig.getoption("pixi_exec"))

    # Check pixi environment variable
    project_root = os.environ.get("PIXI_PROJECT_ROOT")
    if not project_root:
        pytest.exit("PROJECT_ROOT environment variable is not set, run from pixi task")

    # Check if the target directory exists
    # This assertion is for the type checker
    assert project_root
    target_dir = pathlib.Path(project_root).joinpath("target/pixi/release")
    if not target_dir.exists():
        pytest.exit("pixi executable not found, run `pixi r build` first")

    if sys.platform.startswith("win"):
        return target_dir.joinpath("pixi.exe")
    else:
        return target_dir.joinpath("pixi")
