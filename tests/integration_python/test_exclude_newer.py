from pathlib import Path

import tomli

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command


def exclude_newer(manifest: Path, key: str = "exclude-newer") -> object:
    return tomli.loads(manifest.read_text())["workspace"].get(key)


def test_exclude_newer_cli(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    """The exclude-newer commands edit the manifest and keep the lock file in sync."""
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f"""
[workspace]
name = "test"
channels = ["{dummy_channel_1}"]
platforms = ["{CURRENT_PLATFORM}"]

[dependencies]
dummy-b = "*"
""")
    flags: list[str | Path] = ["--manifest-path", manifest]
    verify_cli_command([pixi, "lock", *flags])
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", *flags], stderr_contains="No exclude-newer"
    )

    # Every package in dummy_channel_1 was uploaded in 2025, so the lock file
    # cannot be updated and the manifest stays untouched.
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "set", "2020-01-01", "--no-install", *flags],
        ExitCode.FAILURE,
        stderr_contains="uploaded after the cutoff date",
    )
    assert exclude_newer(manifest) is None

    # Exemptions require a cutoff
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "exempt", "dummy-b", "--no-install", *flags],
        ExitCode.FAILURE,
        stderr_contains="requires a cutoff",
    )

    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "set", "2020-01-01", "--frozen", *flags],
    )
    assert exclude_newer(manifest) == "2020-01-01"

    # The exemption keeps the locked package satisfiable
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "exempt", "dummy-b>=0.1", "--no-install", *flags],
        stderr_contains="Exempted dummy-b>=0.1",
    )
    assert exclude_newer(manifest) == {
        "cutoff": "2020-01-01",
        "exemptions": {"dummy-b": ">=0.1"},
    }
    verify_cli_command([pixi, "lock", "--check", *flags])
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "get", *flags],
        stdout_contains='exemptions = { dummy-b = ">=0.1" }',
    )

    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "unset", "--no-install", *flags],
        ExitCode.FAILURE,
        stderr_contains="cannot be removed while exemptions are configured",
    )
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "unexempt", "dummy-a", "--no-install", *flags],
        ExitCode.FAILURE,
        stderr_contains="dummy-a is not exempt",
    )

    # Without the exemption the locked package is excluded again
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "unexempt", "dummy-b", "--no-install", *flags],
        ExitCode.FAILURE,
        stderr_contains="uploaded after the cutoff date",
    )
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "unexempt", "dummy-b", "--frozen", *flags],
    )
    assert exclude_newer(manifest) == "2020-01-01"

    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "unset", "--frozen", *flags],
    )
    assert exclude_newer(manifest) is None


def test_pypi_exclude_newer_cli(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """PyPI exemptions only take package names and work without a cutoff."""
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f"""
[workspace]
name = "test"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
""")
    flags: list[str | Path] = ["--manifest-path", manifest, "--pypi", "--no-install"]

    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "exempt", "torch==2.0", *flags],
        ExitCode.FAILURE,
        stderr_contains="can only name a package",
    )
    verify_cli_command([pixi, "workspace", "exclude-newer", "exempt", "Torch", *flags])
    assert exclude_newer(manifest, "pypi-exclude-newer") == {"exemptions": {"Torch": "*"}}

    verify_cli_command([pixi, "workspace", "exclude-newer", "set", "1d", *flags])
    verify_cli_command([pixi, "workspace", "exclude-newer", "unset", *flags])
    verify_cli_command([pixi, "workspace", "exclude-newer", "unexempt", "torch", *flags])
    assert exclude_newer(manifest, "pypi-exclude-newer") is None
