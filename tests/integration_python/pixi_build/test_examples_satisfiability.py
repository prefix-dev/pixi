import tomllib
from pathlib import Path

import pytest

from .common import repo_root, verify_cli_command


def workspace_example_manifests() -> list[Path]:
    """Collect the workspace manifests of all examples.

    Globs for `pixi.toml` and `pyproject.toml` files in `examples/` and keeps
    only the ones that describe an actual pixi workspace.
    """
    manifests: list[Path] = []
    for manifest_path in sorted(repo_root().joinpath("examples").glob("**/p*.toml")):
        manifest = tomllib.loads(manifest_path.read_text())
        if manifest_path.name == "pyproject.toml":
            # Only consider pyproject.toml files that configure a pixi workspace
            # to avoid testing non-pixi files.
            workspace = manifest.get("tool", {}).get("pixi", {}).get("workspace")
        elif manifest_path.name == "pixi.toml":
            # Only consider pixi.toml files with a workspace section to avoid
            # testing non-workspace member manifests.
            workspace = manifest.get("workspace")
        else:
            continue
        if workspace is not None:
            manifests.append(manifest_path)
    return manifests


WORKSPACE_EXAMPLE_MANIFESTS = workspace_example_manifests()


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
