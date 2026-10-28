import sys
from pathlib import Path
from typing import Any, cast

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
        [
            pixi,
            "workspace",
            "exclude-newer",
            "exempt",
            "dummy-b>=0.1",
            "--no-cutoff",
            "--no-install",
            *flags,
        ],
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


def test_exclude_newer_exemption_cutoff_cli(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    """Exemptions with a cutoff only exempt the releases published up to it."""
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
        [pixi, "workspace", "exclude-newer", "set", "2020-01-01", "--frozen", *flags],
    )

    # Every package in dummy_channel_1 was uploaded in 2025, after the cutoff
    # of the exemption.
    verify_cli_command(
        [
            pixi,
            "workspace",
            "exclude-newer",
            "exempt",
            "dummy-b",
            "--cutoff",
            "2020-06-01",
            "--no-install",
            *flags,
        ],
        ExitCode.FAILURE,
        stderr_contains="uploaded after the cutoff date",
    )
    assert exclude_newer(manifest) == "2020-01-01"

    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "exempt", "dummy-b", "--cutoff", "7d", *flags],
        ExitCode.FAILURE,
        stderr_contains="not a valid exemption cutoff",
    )
    verify_cli_command(
        [
            pixi,
            "workspace",
            "exclude-newer",
            "exempt",
            "dummy-b",
            "--cutoff",
            "now",
            "--pypi",
            *flags,
        ],
        ExitCode.FAILURE,
        stderr_contains="PyPI exemptions do not support a cutoff",
    )
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "exempt", "dummy-b", "--no-cutoff", "--pypi", *flags],
        ExitCode.FAILURE,
        stderr_contains="PyPI exemptions do not support a cutoff",
    )

    # A spec with a version defaults to a cutoff of now, a bare name to none
    verify_cli_command(
        [
            pixi,
            "workspace",
            "exclude-newer",
            "exempt",
            "dummy-b==0.1.0",
            "dummy-a",
            "--no-install",
            *flags,
        ],
        stderr_contains="Exempted dummy-b==0.1.0 (published up to ",
    )
    exemptions = cast(dict[str, Any], exclude_newer(manifest))["exemptions"]
    assert exemptions["dummy-a"] == "*"
    assert exemptions["dummy-b"]["version"] == "==0.1.0"
    cutoff = exemptions["dummy-b"]["cutoff"]
    verify_cli_command([pixi, "lock", "--check", *flags])
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "get", *flags],
        stdout_contains=f'dummy-b = {{ version = "==0.1.0", cutoff = "{cutoff}" }}',
    )

    # Moving the cutoff of an exemption later warns, moving it earlier doesn't
    moved = "moved the exclude-newer exemption cutoff of dummy-b"
    for args, stderr in [(["--no-cutoff"], moved), (["--cutoff", cutoff], None)]:
        verify_cli_command(
            [
                pixi,
                "workspace",
                "exclude-newer",
                "exempt",
                "dummy-b==0.1.0",
                *args,
                "--no-install",
                *flags,
            ],
            stderr_contains=stderr,
            stderr_excludes=None if stderr else moved,
        )
    exemptions = cast(dict[str, Any], exclude_newer(manifest))["exemptions"]
    assert exemptions["dummy-b"]["cutoff"] == cutoff

    # Nothing is stale while the workspace cutoff is before the exemption cutoff
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "prune", *flags],
        stderr_contains="There are no stale exclude-newer exemptions",
    )

    # Once the workspace cutoff passes it, the exemption is stale
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "set", "2100-01-01", "--frozen", *flags],
    )
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "prune", "--dry-run", *flags],
        stderr_contains="Would remove the stale exclude-newer exemptions of dummy-b",
    )
    assert "dummy-b" in cast(dict[str, Any], exclude_newer(manifest))["exemptions"]
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "prune", *flags],
        stderr_contains="Removed the stale exclude-newer exemptions of dummy-b",
    )
    # The exemption without a cutoff is never stale
    assert exclude_newer(manifest) == {
        "cutoff": "2100-01-01",
        "exemptions": {"dummy-a": "*"},
    }


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

    verify_cli_command([pixi, "workspace", "exclude-newer", "set", "2020-01-01T00:00:00Z", *flags])
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "get", "--manifest-path", manifest],
        stdout_contains='cutoff = "2020-01-01T00:00:00Z"',
    )
    verify_cli_command([pixi, "workspace", "exclude-newer", "set", "1d", *flags])
    verify_cli_command([pixi, "workspace", "exclude-newer", "unset", *flags])
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "unset", *flags], stderr_contains="unchanged"
    )
    verify_cli_command([pixi, "workspace", "exclude-newer", "unexempt", "torch", *flags])
    assert exclude_newer(manifest, "pypi-exclude-newer") is None


def test_exclude_newer_cutoff_as_written(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Cutoffs are compared and shown as written, not as the instant they mean."""
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f"""
[workspace]
name = "test"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
exclude-newer = "7d" # one week
""")
    flags: list[str | Path] = ["--manifest-path", manifest]
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", *flags], stdout_contains='exclude-newer = "7d"'
    )
    verify_cli_command(
        [pixi, "workspace", "exclude-newer", "set", "7d", "--no-install", *flags],
        stderr_contains="unchanged",
    )
    verify_cli_command([pixi, "workspace", "exclude-newer", "set", "168h", "--no-install", *flags])
    assert 'exclude-newer = "168h" # one week' in manifest.read_text()


def test_config_exclude_newer_init(pixi: Path, tmp_path: Path) -> None:
    """`pixi init` writes the `exclude-newer` of the configuration."""
    env = {"PIXI_HOME": str(tmp_path / "home")}
    (tmp_path / "home").mkdir()
    (tmp_path / "home" / "config.toml").write_text(
        '[exclude-newer]\ncutoff = "7d"\nexemptions = { dummy-b = "*" }\n'
    )

    workspace = tmp_path / "workspace"
    verify_cli_command([pixi, "init", workspace], env=env)
    assert exclude_newer(workspace / "pixi.toml") == {
        "cutoff": "7d",
        "exemptions": {"dummy-b": "*"},
    }


def test_config_exclude_newer_exec(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """`pixi exec` solves with the `exclude-newer` of the configuration."""
    env = {"PIXI_HOME": str(tmp_path)}
    config = tmp_path / "config.toml"
    # The dummy packages ship no executable that runs on Windows, so install
    # dummy-g as a spec and run a command that exists on every platform.
    noop = ["cmd", "/c", "exit 0"] if sys.platform.startswith("win") else ["true"]
    command: list[str | Path] = [
        pixi,
        "exec",
        "--channel",
        dummy_channel_1,
        "--list",
        "--spec",
        "dummy-g",
        "--",
        *noop,
    ]

    # Every package in dummy_channel_1 was uploaded in 2025.
    config.write_text('exclude-newer = "2020-01-01"\n')
    verify_cli_command(
        command, ExitCode.FAILURE, env=env, stderr_contains="uploaded after the cutoff date"
    )

    # dummy-g depends on dummy-b.
    config.write_text(
        'exclude-newer = { cutoff = "2020-01-01", exemptions = { dummy-g = "*", dummy-b = "*" } }\n'
    )
    verify_cli_command(command, env=env, stdout_contains=["dummy-g", "dummy-b"])


def test_config_exclude_newer_global(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """A new global manifest starts from the `exclude-newer` of the
    configuration."""
    env = {"PIXI_HOME": str(tmp_path)}
    (tmp_path / "config.toml").write_text(
        'exclude-newer = { cutoff = "2020-01-01", exemptions = { dummy-b = "*" } }\n'
    )

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-b"], env=env
    )
    manifest = tomli.loads((tmp_path / "manifests" / "pixi-global.toml").read_text())
    assert list(manifest)[:2] == ["version", "exclude-newer"]
    assert manifest["exclude-newer"] == {
        "cutoff": "2020-01-01",
        "exemptions": {"dummy-b": "*"},
    }

    # The cutoff applies to the packages that are not exempted.
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-g"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="uploaded after the cutoff date",
    )


def test_config_exclude_newer_invalid_global(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """An invalid `exclude-newer` in the configuration doesn't leave behind a
    global manifest that never gets it."""
    env = {"PIXI_HOME": str(tmp_path)}
    config = tmp_path / "config.toml"
    config.write_text('exclude-newer = "garbage"\n')
    command: list[str | Path] = [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-b"]

    verify_cli_command(command, ExitCode.FAILURE, env=env)
    manifest = tmp_path / "manifests" / "pixi-global.toml"
    assert not manifest.exists()

    config.write_text('exclude-newer = "2020-01-01"\n')
    verify_cli_command(
        command, ExitCode.FAILURE, env=env, stderr_contains="uploaded after the cutoff date"
    )
    assert tomli.loads(manifest.read_text())["exclude-newer"] == "2020-01-01"
