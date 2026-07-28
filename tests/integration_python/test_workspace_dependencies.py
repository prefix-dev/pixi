"""Edge-case tests for `{ workspace = true }` in environment dependency tables."""

import json
import tomllib
from pathlib import Path

from .common import (
    CURRENT_PLATFORM,
    ExitCode,
    verify_cli_command,
)


def build_manifest(
    channel: str,
    workspace_dependencies: str,
    body: str,
) -> str:
    return f"""
[workspace]
name = "test-workspace-deps"
channels = ["{channel}"]
platforms = ["{CURRENT_PLATFORM}"]

[workspace.dependencies]
{workspace_dependencies}

{body}
"""


def test_lock_resolves_inherited_entries_across_tables(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    """Markers in every environment table resolve against the pool."""
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"\npackage2 = "==0.1.0"',
            f"""
[dependencies]
package = {{ workspace = true }}

[constraints]
package2 = {{ workspace = true }}

[feature.dev.dependencies]
package2 = {{ workspace = true }}

[target.{CURRENT_PLATFORM}.dependencies]
package4 = "==0.1.0"

[environments]
dev = ["dev"]
""",
        )
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest_path])
    lock_file = (tmp_pixi_workspace / "pixi.lock").read_text()
    assert "package-0.1.0-" in lock_file
    assert "package2-0.1.0-" in lock_file


def test_lock_resolves_inherited_entry_in_feature_target_table(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            f"""
[feature.dev.target.{CURRENT_PLATFORM}.dependencies]
package = {{ workspace = true }}

[environments]
dev = ["dev"]
""",
        )
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest_path])
    assert "package-0.1.0-" in (tmp_pixi_workspace / "pixi.lock").read_text()


def test_lock_resolves_inherited_entries_in_legacy_host_and_build_tables(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    """The legacy workspace-level host/build tables participate without pixi-build."""
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"\npackage2 = "==0.1.0"',
            """
[host-dependencies]
package = { workspace = true }

[build-dependencies]
package2 = { workspace = true }
""",
        )
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest_path])


def test_pool_lookup_is_case_insensitive(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'PACKAGE = "==0.1.0"',
            """
[dependencies]
package = { workspace = true }
""",
        )
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest_path])


def test_override_layers_on_inherited_entry(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    """A build override next to the marker narrows the inherited spec."""
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    # package3 has the build string "abc" on every platform.
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package3 = "==0.1.0"',
            """
[dependencies]
package3 = { workspace = true, build = "ab*" }
""",
        )
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest_path])

    # A build override that matches nothing must fail the solve.
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package3 = "==0.1.0"',
            """
[dependencies]
package3 = { workspace = true, build = "nonexistent*" }
""",
        )
    )
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest_path],
        ExitCode.FAILURE,
    )


def test_missing_pool_entry_fails(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
ghost = { workspace = true }
""",
        )
    )
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest_path],
        ExitCode.FAILURE,
        stderr_contains="does not define `ghost` in `[workspace.dependencies]`",
    )


def test_workspace_false_fails(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
package = { workspace = false }
""",
        )
    )
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest_path],
        ExitCode.FAILURE,
        stderr_contains="`workspace` cannot be false",
    )


def test_version_override_on_marker_fails(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
package = { workspace = true, version = "==0.2.0" }
""",
        )
    )
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest_path],
        ExitCode.FAILURE,
        stderr_contains="`version` is mutually exclusive with `workspace`",
    )


def test_source_location_override_on_marker_fails(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    """The workspace entry owns the source location; it cannot be overridden."""
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    for field, spec in [
        ("path", 'path = "./lib"'),
        ("git", 'git = "https://github.com/user/repo.git"'),
        ("url", 'url = "https://example.com/pkg.conda"'),
    ]:
        manifest_path.write_text(
            build_manifest(
                multiple_versions_channel_1,
                'package = "==0.1.0"',
                f"""
[dependencies]
package = {{ workspace = true, {spec} }}
""",
            )
        )
        verify_cli_command(
            [pixi, "lock", "--manifest-path", manifest_path],
            ExitCode.FAILURE,
            stderr_contains=f"`{field}` is mutually exclusive with `workspace`",
        )


def test_source_pool_entry_requires_pixi_build(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'mylib = { path = "libs/mylib" }',
            """
[dependencies]
mylib = { workspace = true }
""",
        )
    )
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest_path],
        ExitCode.FAILURE,
        stderr_contains="pixi-build",
    )


def test_add_bare_keeps_marker_and_hints_at_workspace(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
package = { workspace = true }
""",
        )
    )
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "package"],
        stderr_contains="inherits from `[workspace.dependencies]`",
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["dependencies"]["package"] == {"workspace": True}


def test_add_explicit_spec_replaces_marker(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
package = { workspace = true }
""",
        )
    )
    verify_cli_command([pixi, "add", "--manifest-path", manifest_path, "package==0.2.0"])
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["dependencies"]["package"] == "==0.2.0"
    # The pool entry itself stays untouched.
    assert parsed_manifest["workspace"]["dependencies"]["package"] == "==0.1.0"


def test_add_bare_keeps_marker_in_feature_table(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[feature.dev.dependencies]
package = { workspace = true }

[environments]
dev = ["dev"]
""",
        )
    )
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "--feature", "dev", "package"],
        stderr_contains="inherits from `[workspace.dependencies]`",
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["feature"]["dev"]["dependencies"]["package"] == {"workspace": True}


def test_upgrade_keeps_markers_in_feature_and_target_tables(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"\npackage2 = "==0.1.0"',
            f"""
[feature.dev.dependencies]
package = {{ workspace = true }}

[target.{CURRENT_PLATFORM}.dependencies]
package2 = {{ workspace = true }}

[environments]
dev = ["dev"]
""",
        )
    )
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path],
        stderr_contains="[workspace.dependencies]",
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["feature"]["dev"]["dependencies"]["package"] == {"workspace": True}
    assert parsed_manifest["target"][CURRENT_PLATFORM]["dependencies"]["package2"] == {
        "workspace": True
    }
    assert parsed_manifest["workspace"]["dependencies"]["package"] == "==0.1.0"
    assert parsed_manifest["workspace"]["dependencies"]["package2"] == "==0.1.0"


def test_upgrade_keeps_dotted_marker(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    """The dotted `package.workspace = true` form is recognized as well."""
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
package.workspace = true
""",
        )
    )
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path],
        stderr_contains="[workspace.dependencies]",
    )
    assert "package.workspace = true" in manifest_path.read_text()


def test_upgrade_json_suppresses_inherited_note(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    """The inherited-entries note is quiet in JSON mode like all other output."""
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
package = { workspace = true }
""",
        )
    )
    output = verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "--json"],
        stderr_excludes="[workspace.dependencies]",
    )
    json.loads(output.stdout)
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["dependencies"]["package"] == {"workspace": True}


def test_remove_inherited_entry(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
package = { workspace = true }
""",
        )
    )
    verify_cli_command([pixi, "remove", "--manifest-path", manifest_path, "package"])
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert "package" not in parsed_manifest.get("dependencies", {})
    # The pool entry itself stays untouched.
    assert parsed_manifest["workspace"]["dependencies"]["package"] == "==0.1.0"


def test_pyproject_add_and_upgrade_keep_marker(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    """The marker also works in the `[tool.pixi.*]` tables of a pyproject.toml."""
    manifest_path = tmp_pixi_workspace / "pyproject.toml"
    manifest_path.write_text(
        f"""
[project]
name = "test-workspace-deps"
version = "0.1.0"

[tool.pixi.workspace]
channels = ["{multiple_versions_channel_1}"]
platforms = ["{CURRENT_PLATFORM}"]

[tool.pixi.workspace.dependencies]
package = "==0.1.0"

[tool.pixi.dependencies]
package = {{ workspace = true }}
"""
    )
    # `--frozen` skips the solve; a pyproject manifest implicitly requires
    # python, which the test channel does not provide.
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "--frozen", "package"],
        stderr_contains="inherits from `[workspace.dependencies]`",
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["tool"]["pixi"]["dependencies"]["package"] == {"workspace": True}

    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "--frozen", "package==0.2.0"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["tool"]["pixi"]["dependencies"]["package"] == "==0.2.0"


def test_add_platform_specific_does_not_touch_root_marker(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    """A platform-scoped add writes to the target table and leaves the root marker alone."""
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest_path.write_text(
        build_manifest(
            multiple_versions_channel_1,
            'package = "==0.1.0"',
            """
[dependencies]
package = { workspace = true }
""",
        )
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--platform",
            CURRENT_PLATFORM,
            "package==0.2.0",
        ]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["dependencies"]["package"] == {"workspace": True}
    assert parsed_manifest["target"][CURRENT_PLATFORM]["dependencies"]["package"] == "==0.2.0"
