import tomllib
from pathlib import Path
from typing import TypeAlias, cast

import pytest

from .common import current_platform, repo_root, verify_cli_command

Workspace: TypeAlias = dict[str, object]


def workspace_from_manifest(manifest_path: Path) -> Workspace | None:
    manifest = cast(dict[str, object], tomllib.loads(manifest_path.read_text()))
    if manifest_path.name == "pyproject.toml":
        # Only consider pyproject.toml files that configure a pixi workspace
        # to avoid testing non-pixi files.
        tool = cast(dict[str, object], manifest.get("tool", {}))
        pixi = cast(dict[str, object], tool.get("pixi", {}))
        workspace = pixi.get("workspace")
    elif manifest_path.name == "pixi.toml":
        # Only consider pixi.toml files with a workspace section to avoid
        # testing non-workspace member manifests.
        workspace = manifest.get("workspace")
    else:
        return None

    if isinstance(workspace, dict):
        return cast(Workspace, workspace)
    return None


def workspace_example_manifests() -> list[Path]:
    """Collect the workspace manifests of all examples.

    Globs for `pixi.toml` and `pyproject.toml` files in `examples/` and keeps
    only the ones that describe an actual pixi workspace.
    """
    manifests: list[Path] = []
    for manifest_path in sorted(repo_root().joinpath("examples").glob("**/p*.toml")):
        if workspace_from_manifest(manifest_path) is not None:
            manifests.append(manifest_path)
    return manifests


WORKSPACE_EXAMPLE_MANIFESTS = workspace_example_manifests()


def supports_current_platform(workspace: Workspace) -> bool:
    """Return whether the workspace supports the current runner platform."""
    platforms = workspace.get("platforms")
    if not isinstance(platforms, list):
        return False

    current = current_platform()
    for platform in cast(list[object], platforms):
        if platform == current:
            return True
        if isinstance(platform, dict):
            rich_platform = cast(dict[str, object], platform)
            if rich_platform.get("name") == current or rich_platform.get("platform") == current:
                return True

    return False


@pytest.mark.slow
@pytest.mark.parametrize(
    "manifest_path",
    WORKSPACE_EXAMPLE_MANIFESTS,
    ids=[str(p.relative_to(repo_root())) for p in WORKSPACE_EXAMPLE_MANIFESTS],
)
def test_example_lock_file_satisfiability(pixi: Path, manifest_path: Path) -> None:
    """Verify that the committed lock file of each example satisfies its manifest.

    `pixi tree --locked` performs the satisfiability check without re-solving or
    installing the environment. The build backends are provided through the
    `PIXI_BUILD_BACKEND_OVERRIDE` that is set by the session fixture in
    `conftest.py`.
    """
    workspace = workspace_from_manifest(manifest_path)
    if workspace is None:
        pytest.fail(f"{manifest_path} does not contain a pixi workspace")

    if not supports_current_platform(workspace):
        pytest.skip(f"example does not support current platform {current_platform()}")

    verify_cli_command(
        [
            pixi,
            "tree",
            "--locked",
            "--no-install",
            "--manifest-path",
            manifest_path,
        ],
    )
