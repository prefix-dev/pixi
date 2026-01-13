"""Build-specific test utilities for pixi-build tests."""

import os
import shutil
import subprocess
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from collections.abc import Generator
from typing import Any

import tomli_w
import yaml

# Re-export from parent common module
from ..common import (
    ALL_PLATFORMS,
    CONDA_FORGE_CHANNEL,
    CURRENT_PLATFORM,
    EMPTY_BOILERPLATE_PROJECT,
    ExitCode,
    Output,
    bat_extension,
    current_platform,
    default_env_path,
    exec_extension,
    get_manifest,
    is_binary,
    pixi_dir,
    repo_root,
    verify_cli_command,
)

__all__ = [
    # Re-exports
    "ALL_PLATFORMS",
    "CONDA_FORGE_CHANNEL",
    "CURRENT_PLATFORM",
    "EMPTY_BOILERPLATE_PROJECT",
    "ExitCode",
    "Output",
    "bat_extension",
    "current_platform",
    "default_env_path",
    "exec_extension",
    "get_manifest",
    "is_binary",
    "pixi_dir",
    "repo_root",
    "verify_cli_command",
    # Build-specific
    "Workspace",
    "copy_manifest",
    "copytree_with_local_backend",
    "cwd",
    "git_test_repo",
]


@dataclass
class Workspace:
    """Represents a pixi workspace for build tests."""

    recipe: dict[str, Any]
    workspace_manifest: dict[str, Any]
    workspace_dir: Path
    package_manifest: dict[str, Any]
    package_dir: Path
    recipe_path: Path
    debug_dir: Path

    def write_files(self) -> None:
        self.recipe_path.write_text(yaml.dump(self.recipe))
        workspace_manifest_path = self.workspace_dir.joinpath("pixi.toml")
        workspace_manifest_path.write_text(tomli_w.dumps(self.workspace_manifest))
        package_manifest_path = self.package_dir.joinpath("pixi.toml")
        package_manifest_path.write_text(tomli_w.dumps(self.package_manifest))

    def iter_debug_dirs(self) -> list[Path]:
        candidates: list[Path] = []
        work_root = self.workspace_dir.joinpath(".pixi", "build", "work")
        if work_root.is_dir():
            for entry in sorted(work_root.iterdir()):
                debug_candidate = entry.joinpath("debug")
                if debug_candidate.is_dir():
                    candidates.append(debug_candidate)
        return candidates

    def find_debug_file(self, filename: str) -> Path | None:
        for debug_dir in self.iter_debug_dirs():
            target = debug_dir.joinpath(filename)
            if target.is_file():
                return target
        return None


def copy_manifest(
    src: os.PathLike[str],
    dst: os.PathLike[str],
) -> Path:
    """Copy file (simple copy, no channel rewriting needed in merged repo)."""
    return Path(shutil.copy(src, dst))


def copytree_with_local_backend(
    src: os.PathLike[str],
    dst: os.PathLike[str],
    **kwargs: Any,
) -> Path:
    """Copy tree while ignoring .pixi directories and .conda files."""
    kwargs.setdefault("copy_function", copy_manifest)

    return Path(
        shutil.copytree(src, dst, ignore=shutil.ignore_patterns(".pixi", "*.conda"), **kwargs)
    )


@contextmanager
def cwd(path: str | Path) -> Generator[None, None, None]:
    """Context manager to temporarily change the current working directory."""
    oldpwd = os.getcwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(oldpwd)


def git_test_repo(source_dir: Path, repo_name: str, target_dir: Path) -> str:
    """Create a git repository from the source directory in a target directory."""
    repo_path: Path = target_dir / repo_name

    # Copy source directory to temp
    copytree_with_local_backend(source_dir, repo_path, copy_function=copy_manifest)

    # Initialize git repository in the copied source
    subprocess.run(
        ["git", "init"],
        cwd=repo_path,
        check=True,
        capture_output=True,
    )

    # Add all files and commit
    subprocess.run(
        ["git", "add", "."],
        cwd=repo_path,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.email", "bot@prefix.dev"],
        cwd=repo_path,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "Bot"],
        cwd=repo_path,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "commit", "--message", "Initial commit"],
        cwd=repo_path,
        check=True,
        capture_output=True,
    )

    return f"file://{repo_path}"
