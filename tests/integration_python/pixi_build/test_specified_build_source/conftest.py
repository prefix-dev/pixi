import subprocess
from dataclasses import dataclass
from pathlib import Path

import pytest

from ..common import copytree_with_local_backend


@dataclass(frozen=True)
class LocalGitRepo:
    path: Path
    main_rev: str
    other_feature_rev: str
    tag: str


@pytest.fixture
def local_cpp_git_repo(
    pixi: Path,
    build_data: Path,
    tmp_path_factory: pytest.TempPathFactory,
) -> LocalGitRepo:
    """
    Create a local git repository mirroring the minimal pixi-build-cmake workspace so tests can
    exercise git sources without touching the network.
    """

    source_root = build_data.joinpath("minimal-backend-workspaces", "pixi-build-cmake")
    repo_root = tmp_path_factory.mktemp("git-repo")
    repo_path = repo_root.joinpath("repo")
    copytree_with_local_backend(source_root, repo_path)

    marker = repo_path.joinpath("src", "LOCAL_MARKER.txt")
    marker.write_text("local git fixture marker\n", encoding="utf-8")

    main_source_path = repo_path.joinpath("src", "main.cpp")
    original_source = main_source_path.read_text(encoding="utf-8")

    def run_git(*args: str) -> str:
        result = subprocess.run(
            [str(pixi), "run", "git", *args],
            cwd=repo_path,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise RuntimeError(
                "git command failed ({}):\nstdout: {}\nstderr: {}".format(
                    " ".join(args), result.stdout, result.stderr
                )
            )
        return result.stdout.strip()

    run_git("init", "-b", "main")
    run_git("config", "user.email", "pixi-tests@example.com")
    run_git("config", "user.name", "Pixi Build Tests")
    run_git("add", ".")
    run_git("commit", "-m", "Initial commit")

    run_git("checkout", "-b", "other-feature")
    feature_text = original_source.replace(
        "Build backend works", "Build backend works from other-feature branch"
    )
    if feature_text == original_source:
        feature_text = original_source + "\n// other-feature branch tweak\n"
    main_source_path.write_text(feature_text)
    run_git("add", main_source_path.relative_to(repo_path).as_posix())
    run_git("commit", "-m", "Add branch change")
    other_feature_rev = run_git("rev-parse", "HEAD")

    run_git("checkout", "main")
    main_update_text = original_source.replace(
        "Build backend works", "Build backend works on main branch"
    )
    if main_update_text == original_source:
        main_update_text = original_source + "\n// main branch tweak\n"
    main_source_path.write_text(main_update_text)
    run_git("add", main_source_path.relative_to(repo_path).as_posix())
    run_git("commit", "-m", "Update main")
    main_rev = run_git("rev-parse", "HEAD")

    run_git("tag", "fixture-v1")

    return LocalGitRepo(
        path=repo_path,
        main_rev=main_rev,
        other_feature_rev=other_feature_rev,
        tag="fixture-v1",
    )
