import os
import shutil
import stat
from pathlib import Path
from typing import Any
from collections.abc import Callable

import pytest

from .common import (
    CURRENT_PLATFORM,
    copy_manifest,
    copytree_with_local_backend,
    exec_extension,
    git_test_repo,
    verify_cli_command,
)

MINIMAL_GIT_PACKAGE_MANIFEST = f"""
[workspace]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["{CURRENT_PLATFORM}"]
preview = ["pixi-build"]

[package]
name = "simple-app"
version = "0.1.0"

[package.build.backend]
channels = [
  "https://prefix.dev/pixi-build-backends",
  "https://prefix.dev/conda-forge",
]
name = "pixi-build-rattler-build"
version = "*"
"""


def rmtree_force(path: Path) -> None:
    """`shutil.rmtree` that copes with git's read-only object files.

    Git marks everything under `.git/objects` read-only, which on Windows
    makes `unlink` fail with `Access is denied` rather than honouring the
    parent directory's permissions as on unix.
    """

    def on_error(func: Callable[..., Any], target: str, _exc: BaseException) -> None:
        os.chmod(target, stat.S_IWRITE)
        func(target)

    shutil.rmtree(path, onexc=on_error)


MINIMAL_GIT_PACKAGE_RECIPE = """
package:
  name: simple-app
  version: 0.1.0

source:
  path: .
  use_gitignore: true

build:
  number: 0
  script:
    - if: win
      then:
        - if not exist %PREFIX%\\bin mkdir %PREFIX%\\bin
        - echo @echo off > %PREFIX%\\bin\\simple-app.bat
        - echo echo Build backend works >> %PREFIX%\\bin\\simple-app.bat
      else:
        - mkdir -p $PREFIX/bin
        - echo "#!/usr/bin/env bash" > $PREFIX/bin/simple-app
        - echo "echo Build backend works" >> $PREFIX/bin/simple-app
        - chmod +x $PREFIX/bin/simple-app
"""


@pytest.mark.slow
def test_build_git_source_deps(pixi: Path, tmp_pixi_workspace: Path, build_data: Path) -> None:
    """
    This one tries to build the rich example project
    """

    project = build_data / "rich_example"
    target_git_dir = tmp_pixi_workspace / "git_project"
    copytree_with_local_backend(project, target_git_dir)

    # init it as a git repo and commit all files
    verify_cli_command(["git", "init"], cwd=target_git_dir)
    # set some identity
    verify_cli_command(["git", "config", "user.email", "some@email.com"], cwd=target_git_dir)
    verify_cli_command(["git", "config", "user.name", "some-name"], cwd=target_git_dir)

    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "initial commit"], cwd=target_git_dir)

    # extract exact commit hash that we will use
    commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    minimal_workspace = tmp_pixi_workspace / "minimal_workspace"
    minimal_workspace.mkdir()
    copy_manifest(build_data / "manifests" / "workspace_git.toml", minimal_workspace / "pixi.toml")

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    target_git_url = target_git_dir.as_uri()

    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file:///", target_git_url)
    )

    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("CURRENT_PLATFORM", CURRENT_PLATFORM)
    )

    # build it
    verify_cli_command([pixi, "install", "-v", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    assert f"@ git+{target_git_url}#{commit_hash}" in pixi_lock_file.read_text()

    # now we update source code so we can verify that
    # both pixi-git will discover a new commit
    # and pixi build will rebuild it

    rich_example = target_git_dir / "src" / "rich_example" / "__init__.py"
    rich_example.write_text(rich_example.read_text().replace("John Doe", "John Doe Jr."))
    # commit the change
    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "update John Doe"], cwd=target_git_dir)

    # extract updated commit hash that we will use
    new_commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    # build it again
    verify_cli_command([pixi, "update", "-v", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    assert f"@ git+{target_git_url}#{new_commit_hash}" in pixi_lock_file.read_text()

    # run the *built* script to verify that new name is used
    verify_cli_command(
        [pixi, "run", "rich-example-main", "--manifest-path", minimal_workspace / "pixi.toml"],
        stdout_contains="John Doe Jr.",
        cwd=minimal_workspace,
    )


@pytest.mark.slow
def test_build_git_source_deps_from_branch(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """
    This one tries to build the rich example project
    """

    project = build_data / "rich_example"
    target_git_dir = tmp_pixi_workspace / "git_project"
    copytree_with_local_backend(project, target_git_dir)

    # init it as a git repo and commit all files to a test-branch
    verify_cli_command(["git", "init"], cwd=target_git_dir)
    # set some identity
    verify_cli_command(["git", "config", "user.email", "some@email.com"], cwd=target_git_dir)
    verify_cli_command(["git", "config", "user.name", "some-name"], cwd=target_git_dir)

    verify_cli_command(["git", "checkout", "-b", "test-branch"], cwd=target_git_dir)

    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "initial commit"], cwd=target_git_dir)

    # extract exact commit hash that we will use
    commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    minimal_workspace = tmp_pixi_workspace / "minimal_workspace"
    minimal_workspace.mkdir()
    copy_manifest(build_data / "manifests" / "workspace_git.toml", minimal_workspace / "pixi.toml")

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    target_git_url = target_git_dir.as_uri()

    # Replace the rich_example entry using string manipulation
    original = '[dependencies]\nrich_example = { "git" = "file:///" }'
    replacement = '[dependencies]\nrich_example = { "git" = "file:///", "branch" = "test-branch"}'

    workspace_manifest.write_text(workspace_manifest.read_text().replace(original, replacement))
    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file:///", target_git_url)
    )

    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("CURRENT_PLATFORM", CURRENT_PLATFORM)
    )

    # build it
    verify_cli_command([pixi, "install", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    # verify that we recorded used the branch
    assert f"@ git+{target_git_url}?branch=test-branch#{commit_hash}" in pixi_lock_file.read_text()


@pytest.mark.slow
def test_build_git_source_deps_from_rev(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """
    This one tries to build the rich example project
    """

    project = build_data / "rich_example"
    target_git_dir = tmp_pixi_workspace / "git_project"
    copytree_with_local_backend(project, target_git_dir)

    # init it as a git repo and commit all files to a test-branch
    verify_cli_command(["git", "init"], cwd=target_git_dir)
    # set some identity
    verify_cli_command(["git", "config", "user.email", "some@email.com"], cwd=target_git_dir)
    verify_cli_command(["git", "config", "user.name", "some-name"], cwd=target_git_dir)

    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "initial commit"], cwd=target_git_dir)

    # extract exact commit hash that we will use
    commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    minimal_workspace = tmp_pixi_workspace / "minimal_workspace"
    minimal_workspace.mkdir()
    copy_manifest(build_data / "manifests" / "workspace_git.toml", minimal_workspace / "pixi.toml")

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    # Replace the rich_example entry using string manipulation
    original = '[dependencies]\nrich_example = { "git" = "file:///" }'
    replacement = (
        '[dependencies]\nrich_example = {{ "git" = "file:///", "rev" = "{commit_hash}" }}'.format(
            commit_hash=commit_hash[:7]
        )
    )

    target_git_url = target_git_dir.as_uri()

    workspace_manifest.write_text(workspace_manifest.read_text().replace(original, replacement))
    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file:///", target_git_url)
    )
    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("CURRENT_PLATFORM", CURRENT_PLATFORM)
    )

    # build it
    verify_cli_command([pixi, "install", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    # verify that we recorded used rev but also the full one
    assert (
        f"@ git+{target_git_url}?rev={commit_hash[:7]}#{commit_hash}" in pixi_lock_file.read_text()
    )


@pytest.mark.slow
def test_build_git_source_deps_from_tag(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """
    This one tries to build the rich example project
    """

    project = build_data / "rich_example"
    target_git_dir = tmp_pixi_workspace / "git_project"
    copytree_with_local_backend(project, target_git_dir)

    # init it as a git repo and commit all files to a tag called v1.0.0
    verify_cli_command(["git", "init"], cwd=target_git_dir)
    # set some identity
    verify_cli_command(["git", "config", "user.email", "some@email.com"], cwd=target_git_dir)
    verify_cli_command(["git", "config", "user.name", "some-name"], cwd=target_git_dir)

    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "initial commit"], cwd=target_git_dir)
    verify_cli_command(["git", "tag", "v1.0.0", "-m 'my version 1.0.0"], cwd=target_git_dir)

    # extract exact commit hash that we will use
    commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    minimal_workspace = tmp_pixi_workspace / "minimal_workspace"
    minimal_workspace.mkdir()
    copy_manifest(build_data / "manifests" / "workspace_git.toml", minimal_workspace / "pixi.toml")

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    # Replace the rich_example entry using string manipulation
    original = '[dependencies]\nrich_example = { "git" = "file:///" }'
    replacement = '[dependencies]\nrich_example = { "git" = "file:///", "tag" = "v1.0.0" }'

    workspace_manifest.write_text(workspace_manifest.read_text().replace(original, replacement))
    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file:///", target_git_dir.as_uri())
    )

    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("CURRENT_PLATFORM", CURRENT_PLATFORM)
    )

    # build it
    verify_cli_command([pixi, "install", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    # verify that we recorded used rev but also the full one
    assert f"@ git+{target_git_dir.as_uri()}?tag=v1.0.0#{commit_hash}" in pixi_lock_file.read_text()


@pytest.mark.extra_slow
def test_immutable_git_source_is_reused_without_a_checkout(
    pixi: Path, tmp_pixi_workspace: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A package built from a pinned git commit must come back out of the
    artifact cache with no source available at all.

    The proof is destructive rather than a log assertion: after the first
    build the git checkout cache *and the origin repository itself* are
    deleted, so any attempt to materialise the source has nothing to fetch
    from and must fail. A second install that still succeeds can only have
    reused the cached artifact.

    This is the only coverage the checkout-free path has. Every dispatcher
    integration test installs an in-memory backend, which is a
    `BackendOverride` and therefore has no content-addressed identity to
    validate a cached artifact against, so the path is unreachable there.

    The consuming workspace uses a *relative* `exclude-newer`, which is what
    the majority of workspaces (including pixi's own) do. That resolves to
    `Utc::now() - 7d`, a different instant on every invocation, so this also
    covers the cache key staying stable across processes -- when it did not,
    the second install rebuilt instead of hitting the cache and this test
    failed on the deleted repository.

    An *unrelated* backend is left overridden on purpose. Overriding one
    backend must not disable the checkout-free path for packages built by a
    different, channel-solved one; when it did, this test failed the same way.
    """
    # The session-wide `setup_build_backend_override` fixture points every
    # backend at a workspace-built binary. Keep an unrelated one overridden
    # and release this package's own, so the backend is solved from its
    # channel the way a user's would be -- an overridden backend is a
    # `CommandSpec::System` and correctly declines the checkout-free path.
    monkeypatch.setenv(
        "PIXI_BUILD_BACKEND_OVERRIDE",
        f"pixi-build-rust={pixi.parent / exec_extension('pixi-build-rust')}",
    )
    monkeypatch.delenv("PIXI_BUILD_BACKEND_OVERRIDE_ALL", raising=False)

    # Own the cache so the git checkouts can be deleted without touching the
    # user's. The artifact cache is workspace-local and survives this.
    cache_dir = tmp_pixi_workspace / "cache"
    cache_dir.mkdir()
    monkeypatch.setenv("PIXI_CACHE_DIR", str(cache_dir))

    package_src = tmp_pixi_workspace / "package_src"
    package_src.mkdir()
    package_src.joinpath("pixi.toml").write_text(MINIMAL_GIT_PACKAGE_MANIFEST)
    package_src.joinpath("recipe.yaml").write_text(MINIMAL_GIT_PACKAGE_RECIPE)

    git_url = git_test_repo(package_src, "git_package", tmp_pixi_workspace)
    repo_path = tmp_pixi_workspace / "git_package"

    workspace = tmp_pixi_workspace / "workspace"
    workspace.mkdir()
    manifest = workspace / "pixi.toml"
    manifest.write_text(f"""
[workspace]
channels = ["https://prefix.dev/conda-forge"]
exclude-newer = "7d"
name = "immutable-git-cache"
platforms = ["{CURRENT_PLATFORM}"]
preview = ["pixi-build"]

[dependencies]
simple-app = {{ git = "{git_url}" }}
""")

    # Cold: checks the source out, builds it, and caches the artifact.
    verify_cli_command([pixi, "install", "-v", "--manifest-path", manifest])
    artifact_cache = workspace / ".pixi" / "artifacts-v0"
    assert list(artifact_cache.rglob("*.conda")), (
        "the first install should have cached the built artifact"
    )
    git_checkouts = cache_dir / "git-v0"
    assert git_checkouts.is_dir(), "the first install should have checked the source out"

    # Take the source away: both the checkout and the origin it came from.
    shutil.rmtree(workspace / ".pixi" / "envs")
    rmtree_force(git_checkouts)
    rmtree_force(repo_path)
    assert not repo_path.exists()

    # Warm: nothing to fetch and nothing to check out, so this can only pass
    # by reusing the cached artifact.
    verify_cli_command([pixi, "install", "-v", "--locked", "--manifest-path", manifest])

    assert (workspace / ".pixi" / "envs").is_dir(), "the environment should have been reinstalled"
    recreated = list(git_checkouts.rglob("*")) if git_checkouts.exists() else []
    assert not recreated, (
        f"the second install must not have checked anything out, found {recreated}"
    )
