import os
import pathlib
import subprocess
import tomllib
import tomli_w

from typing import Any


StrPath = str | os.PathLike[str]
LOG_DIR = pathlib.Path(__file__).parent / ".logs"


def run(args: list[StrPath], cwd: StrPath | None = None) -> None:
    """
    Run a subprocess and check the return code
    """
    proc: subprocess.CompletedProcess[bytes] = subprocess.run(
        args, cwd=cwd, capture_output=True, check=False
    )
    proc.check_returncode()


def add_system_requirements(
    manifest_path: pathlib.Path, system_requirements: dict[str, Any]
) -> None:
    """
    Add system requirements to the manifest file
    add something like this:
        [system-requirements]
        libc = { family = "glibc", version = "2.17" }
    to the manifest file.
    """
    with manifest_path.open("rb") as f:
        manifest = tomllib.load(f)
    manifest["system-requirements"] = system_requirements
    with manifest_path.open("wb") as f:
        tomli_w.dump(manifest, f)


def setup_stdout_stderr_logging() -> None:
    """
    Set up the logging directory
    """
    if not LOG_DIR.exists():
        LOG_DIR.mkdir()
    for file in LOG_DIR.iterdir():
        file.unlink()


def log_called_process_error(
    name: str, err: subprocess.CalledProcessError, std_err_only: bool = False
) -> None:
    """
    Log the output of a subprocess that failed
    has the option to log only the stderr
    """
    if not LOG_DIR.exists():
        raise RuntimeError("Call setup_stdout_stderr_logging before logging")

    std_out_log = LOG_DIR / f"{name}.stdout"
    std_err_log = LOG_DIR / f"{name}.stderr"
    if err.returncode != 0:
        if not std_err_only:
            with std_out_log.open("w", encoding="utf-8") as f:
                f.write(err.stdout.decode("uft-8"))
        with std_err_log.open("w", encoding="utf-8") as f:
            f.write(err.stderr.decode("utf-8"))
