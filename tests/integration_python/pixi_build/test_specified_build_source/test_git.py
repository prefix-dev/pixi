import json
import shutil
from pathlib import Path
from typing import Any, Iterator

import pytest
import tomli_w
import tomllib
import yaml

from ..common import (
    ExitCode,
    verify_cli_command,
)
from .conftest import LocalGitRepo


def _git_source_entries(lock_file: Path) -> list[dict[str, Any]]:
    data = yaml.safe_load(lock_file.read_text())

    def iter_entries() -> Any:
        yield from data.get("packages", [])

        for env_cfg in data.get("environments", {}).values():
            for platform_packages in env_cfg.get("packages", {}).values():
                yield from platform_packages

    serialized_sources: list[dict[str, Any]] = []
    for entry in iter_entries():
        if isinstance(entry, dict) and entry.get("conda") == ".":
            package_build_source = entry.get("package_build_source")
            if package_build_source is not None:
                serialized_sources.append(package_build_source)

    assert serialized_sources, (
        "expected at least one git package with package_build_source in pixi.lock"
    )
    return serialized_sources


def extract_git_sources(lock_file: Path) -> list[str]:
    return sorted(json.dumps(source, sort_keys=True) for source in _git_source_entries(lock_file))


def configure_local_git_source(
    workspace_dir: Path,
    repo: LocalGitRepo,
    *,
    rev: str | None = None,
    branch: str | None = None,
    tag: str | None = None,
    subdirectory: str = ".",
) -> None:
    manifest_path = workspace_dir / "pixi.toml"
    manifest = tomllib.loads(manifest_path.read_text())
    source = manifest.setdefault("package", {}).setdefault("build", {}).setdefault("source", {})
    for key in ("branch", "tag", "rev"):
        source.pop(key, None)
    source["git"] = "file://" + str(repo.path.as_posix())
    source["subdirectory"] = subdirectory
    if rev is not None:
        source["rev"] = rev
    if branch is not None:
        source["branch"] = branch
    if tag is not None:
        source["tag"] = tag
    manifest_path.write_text(tomli_w.dumps(manifest))


def prepare_cpp_git_workspace(
    workspace_dir: Path,
    build_data: Path,
    repo: LocalGitRepo,
    **source_kwargs: Any,
) -> None:
    project = build_data.joinpath("minimal-backend-workspaces", "pixi-build-cmake")

    # Remove existing .pixi folders
    shutil.rmtree(project.joinpath(".pixi"), ignore_errors=True)

    # Copy to workspace
    shutil.copytree(project, workspace_dir, dirs_exist_ok=True)

    kwargs = dict(source_kwargs)
    kwargs.setdefault("subdirectory", ".")
    configure_local_git_source(workspace_dir, repo, **kwargs)


@pytest.mark.slow
def test_git_path_build(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    """
    Test git path in `[package.build.source]` using a local repository.
    """
    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, rev=local_cpp_git_repo.main_rev
    )

    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )

    verify_cli_command(
        [
            pixi,
            "build",
            "-v",
            "--path",
            tmp_pixi_workspace,
            "--output-dir",
            tmp_pixi_workspace,
        ],
    )

    built_packages = list(tmp_pixi_workspace.glob("*.conda"))
    assert len(built_packages) == 1


@pytest.mark.slow
def test_git_path_lock_behaviour(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    """Exercise git source locking behaviour when switching manifest branches."""

    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, rev=local_cpp_git_repo.main_rev
    )

    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )

    lock_path = tmp_pixi_workspace / "pixi.lock"
    initial_sources = extract_git_sources(lock_path)

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", tmp_pixi_workspace, "--locked"],
    )
    assert extract_git_sources(lock_path) == initial_sources

    configure_local_git_source(tmp_pixi_workspace, local_cpp_git_repo, branch="other-feature")

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", tmp_pixi_workspace, "--locked"],
        expected_exit_code=ExitCode.FAILURE,
    )
    assert extract_git_sources(lock_path) == initial_sources

    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )
    new_sources = extract_git_sources(lock_path)
    assert new_sources != initial_sources

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", tmp_pixi_workspace, "--locked"],
    )
    assert extract_git_sources(lock_path) == new_sources


@pytest.mark.slow
def test_git_path_lock_update_preserves_git_source(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    """Ensure updating another dependency leaves git package_build_source untouched."""

    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, rev=local_cpp_git_repo.main_rev
    )

    manifest_path = tmp_pixi_workspace / "pixi.toml"
    manifest = tomllib.loads(manifest_path.read_text())
    manifest.setdefault("dependencies", {})["zlib"] = "*"
    manifest_path.write_text(tomli_w.dumps(manifest))

    lock_path = tmp_pixi_workspace / "pixi.lock"
    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )
    initial_sources = extract_git_sources(lock_path)

    verify_cli_command(
        [
            pixi,
            "update",
            "-v",
            "--manifest-path",
            tmp_pixi_workspace,
            "zlib",
        ],
    )

    assert extract_git_sources(lock_path) == initial_sources


@pytest.mark.slow
def test_git_path_lock_branch_records_branch_metadata(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, branch="other-feature"
    )

    lock_path = tmp_pixi_workspace / "pixi.lock"
    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )

    entries = _git_source_entries(lock_path)
    assert {entry.get("branch") for entry in entries} == {"other-feature"}
    assert {entry.get("rev") for entry in entries} == {local_cpp_git_repo.other_feature_rev}
    assert all("tag" not in entry for entry in entries)
    assert {entry.get("subdir") for entry in entries} == {"."}

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", tmp_pixi_workspace, "--locked"],
    )


@pytest.mark.slow
def test_git_path_build_has_absolutely_no_respect_to_lock_file(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, branch="other-feature"
    )

    lock_path = tmp_pixi_workspace / "pixi.lock"
    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )

    initial_sources = extract_git_sources(lock_path)
    assert any(local_cpp_git_repo.other_feature_rev in entry for entry in initial_sources)

    repo_path = local_cpp_git_repo.path
    main_path = repo_path.joinpath("src", "main.cpp")
    verify_cli_command([pixi, "run", "git", "checkout", "other-feature"], cwd=repo_path)
    main_path.write_text(
        main_path.read_text().replace("Build backend works", "Build backend works v2")
    )
    verify_cli_command(
        [pixi, "run", "git", "add", main_path.relative_to(repo_path).as_posix()],
        cwd=repo_path,
    )
    verify_cli_command(
        [pixi, "run", "git", "commit", "-m", "Update branch for pixi build test"],
        cwd=repo_path,
    )
    new_branch_rev = verify_cli_command(
        [pixi, "run", "git", "rev-parse", "HEAD"], cwd=repo_path
    ).stdout.strip()
    assert new_branch_rev != local_cpp_git_repo.other_feature_rev

    verify_cli_command(
        [
            pixi,
            "build",
            "-v",
            "--path",
            tmp_pixi_workspace,
            "--output-dir",
            tmp_pixi_workspace,
        ],
    )

    built_packages = list(tmp_pixi_workspace.glob("*.conda"))
    assert built_packages

    # lock file should remain untouched when running `pixi build`
    assert extract_git_sources(lock_path) == initial_sources

    work_dir = tmp_pixi_workspace / ".pixi" / "build" / "work"
    assert work_dir.exists()

    target_phrase = b"Build backend works v2"
    legacy_phrase = b"Build backend works on main branch"

    def iter_candidate_files() -> Iterator[Path]:
        for path in work_dir.rglob("*"):
            if not path.is_file():
                continue
            if path.suffix in {".o", ".bin", ".txt", ".json"} or "simple-app" in path.name:
                yield path

    found = []
    for candidate in iter_candidate_files():
        try:
            data = candidate.read_bytes()
        except OSError:
            continue
        if target_phrase in data:
            found.append(candidate)
        assert legacy_phrase not in data, f"found legacy string in {candidate}"

    assert found, "expected build artifacts to include updated branch contents"


@pytest.mark.slow
def test_git_path_lock_tag_records_tag_metadata(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, tag=local_cpp_git_repo.tag
    )

    lock_path = tmp_pixi_workspace / "pixi.lock"
    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )

    entries = _git_source_entries(lock_path)
    assert {entry.get("tag") for entry in entries} == {local_cpp_git_repo.tag}
    assert {entry.get("rev") for entry in entries} == {local_cpp_git_repo.main_rev}
    assert all("branch" not in entry for entry in entries)
    assert {entry.get("subdir") for entry in entries} == {"."}

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", tmp_pixi_workspace, "--locked"],
    )


@pytest.mark.slow
def test_git_path_lock_rev_marks_explicit_rev(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, rev=local_cpp_git_repo.main_rev
    )

    lock_path = tmp_pixi_workspace / "pixi.lock"
    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )

    entries = _git_source_entries(lock_path)
    assert {entry.get("rev") for entry in entries} == {local_cpp_git_repo.main_rev}
    assert {entry.get("explicit_rev") for entry in entries} == {True}


@pytest.mark.slow
def test_git_path_lock_consistent_across_platforms(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, rev=local_cpp_git_repo.main_rev
    )

    lock_path = tmp_pixi_workspace / "pixi.lock"
    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )

    serialized = extract_git_sources(lock_path)
    assert len(set(serialized)) == 1


@pytest.mark.slow
def test_git_path_lock_detects_manual_rev_change(
    pixi: Path,
    build_data: Path,
    tmp_pixi_workspace: Path,
    local_cpp_git_repo: LocalGitRepo,
) -> None:
    prepare_cpp_git_workspace(
        tmp_pixi_workspace, build_data, local_cpp_git_repo, rev=local_cpp_git_repo.main_rev
    )

    lock_path = tmp_pixi_workspace / "pixi.lock"
    verify_cli_command(
        [pixi, "lock", "-v", "--manifest-path", tmp_pixi_workspace],
    )

    lock_data = yaml.safe_load(lock_path.read_text())

    def mutate(node: Any) -> None:
        if isinstance(node, dict):
            if node.get("conda") == "." and "package_build_source" in node:
                node["package_build_source"]["rev"] = local_cpp_git_repo.other_feature_rev
            for value in node.values():
                mutate(value)
        elif isinstance(node, list):
            for item in node:
                mutate(item)

    mutate(lock_data)
    lock_path.write_text(yaml.safe_dump(lock_data))

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", tmp_pixi_workspace, "--locked"],
        expected_exit_code=ExitCode.FAILURE,
    )
