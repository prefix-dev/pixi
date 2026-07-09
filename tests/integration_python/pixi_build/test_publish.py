"""Integration tests for multi-package `pixi publish`.

`pixi publish` without `--path` builds every workspace package that opts in
with `publish = true` in its `[package]` section, in dependency order, and
uploads them. Every source dependency of a published package must itself opt
in; the publish fails otherwise. These tests exercise the package discovery,
the closure validation, ordering and upload behavior end to end. They build
real packages and download backends from prefix.dev, so they are all marked
`slow`.
"""

from __future__ import annotations

import shutil
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

import pytest
import tomli_w
import yaml

from .common import CURRENT_PLATFORM, ExitCode, git_test_repo, verify_cli_command


BACKEND_CHANNELS = [
    "https://prefix.dev/pixi-build-backends",
    "https://prefix.dev/conda-forge",
]


@dataclass
class Pkg:
    """A member package, its publish opt-in, and the members it depends on."""

    host: list[str] = field(default_factory=list)
    run: list[str] = field(default_factory=list)
    build: list[str] = field(default_factory=list)
    publish: bool = True


def _module(name: str) -> str:
    return name.replace("-", "_")


def _package_table(name: str, pkg: Pkg, backend: str, *, dep_prefix: str = "../") -> dict:
    backend_name = f"pixi-build-{backend}"
    table: dict = {
        "name": name,
        "version": "1.0.0",
        "publish": pkg.publish,
        "build": {
            "backend": {
                "name": backend_name,
                "channels": BACKEND_CHANNELS,
                "version": "*",
            },
        },
    }
    host: dict = {}
    if backend == "python":
        host["hatchling"] = "*"
    host.update({dep: {"path": f"{dep_prefix}{dep}"} for dep in pkg.host})
    if host:
        table["host-dependencies"] = host
    if pkg.run:
        table["run-dependencies"] = {dep: {"path": f"{dep_prefix}{dep}"} for dep in pkg.run}
    if pkg.build:
        table["build-dependencies"] = {dep: {"path": f"{dep_prefix}{dep}"} for dep in pkg.build}
    return table


def _write_package_sources(pkg_dir: Path, name: str, backend: str) -> None:
    """Write the backend-specific source files a package needs to build."""
    if backend == "python":
        module = _module(name)
        pyproject = {
            "project": {
                "name": name,
                "version": "1.0.0",
                "requires-python": ">= 3.11",
                "dependencies": [],
                "scripts": {name: f"{module}:main"},
            },
            "build-system": {
                "build-backend": "hatchling.build",
                "requires": ["hatchling"],
            },
        }
        pkg_dir.joinpath("pyproject.toml").write_text(tomli_w.dumps(pyproject))
        src = pkg_dir.joinpath("src", module)
        src.mkdir(parents=True, exist_ok=True)
        src.joinpath("__init__.py").write_text(
            f'def main() -> None:\n    print("hello from {name}")\n'
        )
    elif backend == "rattler-build":
        recipe = {"package": {"name": name, "version": "1.0.0"}}
        pkg_dir.joinpath("recipe.yaml").write_text(yaml.dump(recipe))
    else:
        raise ValueError(f"unsupported backend {backend!r}")


def write_workspace(
    root: Path,
    packages: dict[str, Pkg],
    *,
    backend: str = "python",
    root_package: str | None = None,
    workspace_deps: list[str] | None = None,
    channels: list[str] | None = None,
) -> None:
    """Write a multi-package pixi workspace under `root`.

    Each entry in `packages` becomes a member directory `root/<name>` with a
    `pixi.toml` and the backend-specific source files; its `Pkg.publish` flag
    becomes the `publish` key of the `[package]` section. `workspace_deps`
    selects which members are referenced from the workspace `[dependencies]`
    table (defaults to all of them except the root package). When
    `root_package` is given, the workspace manifest also carries a `[package]`
    section so the root itself is a publishable package.
    """
    if workspace_deps is None:
        workspace_deps = [name for name in packages if name != root_package]

    workspace_manifest: dict = {
        "workspace": {
            "channels": channels or ["https://prefix.dev/conda-forge"],
            "preview": ["pixi-build"],
            "platforms": [CURRENT_PLATFORM],
        },
        "dependencies": {dep: {"path": dep} for dep in workspace_deps},
    }

    if root_package is not None:
        workspace_manifest["package"] = _package_table(
            root_package, packages.get(root_package, Pkg()), backend, dep_prefix=""
        )
        _write_package_sources(root, root_package, backend)

    root.joinpath("pixi.toml").write_text(tomli_w.dumps(workspace_manifest))

    for name, pkg in packages.items():
        if name == root_package:
            continue
        pkg_dir = root.joinpath(name)
        pkg_dir.mkdir(parents=True, exist_ok=True)
        manifest = {"package": _package_table(name, pkg, backend)}
        pkg_dir.joinpath("pixi.toml").write_text(tomli_w.dumps(manifest))
        _write_package_sources(pkg_dir, name, backend)


def publish_to_dir(pixi: Path, workspace: Path, *, args: list[str] | None = None):
    """Run `pixi publish` (whole-workspace) from `workspace` into `workspace/dist`."""
    target_dir = workspace.joinpath("dist")
    output = verify_cli_command(
        [pixi, "publish", "--target-dir", str(target_dir), *(args or [])],
        cwd=workspace,
    )
    return output, target_dir


def write_standalone_package(pkg_dir: Path, name: str, *, backend: str = "python") -> None:
    """Write a package that lives outside any workspace under test."""
    pkg_dir.mkdir(parents=True, exist_ok=True)
    manifest = {"package": _package_table(name, Pkg(), backend)}
    pkg_dir.joinpath("pixi.toml").write_text(tomli_w.dumps(manifest))
    _write_package_sources(pkg_dir, name, backend)


def built_package_names(target_dir: Path) -> list[str]:
    """Names of the packages in `target_dir`, derived from the artifact files."""
    return sorted(p.name.split("-1.0.0-")[0] for p in target_dir.glob("*.conda"))


def assert_first_occurrence_order(stderr: str, *names: str) -> None:
    """Assert `names` first appear in `stderr` in the given order.

    All publish output is on stderr and the "Building N package(s):" list is
    the first place the package names show up, so first occurrences reflect
    the build order.
    """
    indexes = [stderr.index(name) for name in names]
    assert indexes == sorted(indexes), stderr


def _nest_workspace(tmp_pixi_workspace: Path, name: str) -> Path:
    """Create a sub-workspace directory that keeps the fixture's pixi config."""
    nested = tmp_pixi_workspace.joinpath(name)
    nested.joinpath(".pixi").mkdir(parents=True)
    config = tmp_pixi_workspace.joinpath(".pixi", "config.toml")
    if config.is_file():
        shutil.copy(config, nested.joinpath(".pixi", "config.toml"))
    return nested


@pytest.mark.slow
def test_all_opted_in_packages_are_published(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Two independent opted-in packages are both built and published."""
    write_workspace(tmp_pixi_workspace, {"alpha": Pkg(), "bravo": Pkg()})
    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["alpha", "bravo"]
    assert "Publishing 2 workspace packages" in output.stderr


@pytest.mark.slow
def test_root_package_is_published_with_members(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A `[package]` in the workspace manifest is published like any member.

    The root package also host-depends on a member, which exercises source
    anchoring of relative paths declared in the root manifest.
    """
    write_workspace(
        tmp_pixi_workspace,
        {
            "rootpkg": Pkg(host=["member-a"]),
            "member-a": Pkg(),
        },
        root_package="rootpkg",
    )
    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["member-a", "rootpkg"]
    assert_first_occurrence_order(output.stderr, "member-a", "rootpkg")


@pytest.mark.slow
def test_non_opted_in_package_is_not_published(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A package without `publish = true` is not published.

    Being referenced from the workspace `[dependencies]` table does not make a
    package part of the publish set; only the `publish` flag does.
    """
    write_workspace(
        tmp_pixi_workspace,
        {"alpha": Pkg(), "orphan": Pkg(publish=False)},
    )
    _, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["alpha"]


@pytest.mark.slow
def test_unpublished_source_dependency_fails(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A source dependency on a package that does not opt in fails the publish.

    Uploading `alpha` while skipping `bravo` would leave the target channel
    with an unsatisfiable dependency, so the whole publish is refused.
    """
    write_workspace(
        tmp_pixi_workspace,
        {"alpha": Pkg(host=["bravo"]), "bravo": Pkg(publish=False)},
    )
    verify_cli_command(
        [pixi, "publish", "--target-dir", str(tmp_pixi_workspace.joinpath("dist"))],
        ExitCode.FAILURE,
        cwd=tmp_pixi_workspace,
        stderr_contains="not part of the publish set",
    )


@pytest.mark.slow
def test_transitive_chain_publishes_in_dependency_order(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """alpha -> bravo -> charlie (host deps) publishes charlie, bravo, alpha."""
    write_workspace(
        tmp_pixi_workspace,
        {
            "alpha": Pkg(host=["bravo"]),
            "bravo": Pkg(host=["charlie"]),
            "charlie": Pkg(),
        },
    )
    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["alpha", "bravo", "charlie"]
    assert_first_occurrence_order(output.stderr, "charlie", "bravo", "alpha")


@pytest.mark.slow
def test_diamond_members_published_once_each(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A diamond (alpha -> bravo+charlie -> delta) publishes each member once."""
    write_workspace(
        tmp_pixi_workspace,
        {
            "alpha": Pkg(host=["bravo", "charlie"]),
            "bravo": Pkg(host=["delta"]),
            "charlie": Pkg(host=["delta"]),
            "delta": Pkg(),
        },
    )
    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["alpha", "bravo", "charlie", "delta"]
    assert_first_occurrence_order(output.stderr, "delta", "bravo", "alpha")
    assert_first_occurrence_order(output.stderr, "delta", "charlie", "alpha")


@pytest.mark.slow
def test_run_dependency_orders_publish(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Run dependencies between members also order the publish."""
    write_workspace(
        tmp_pixi_workspace,
        {"alpha": Pkg(run=["bravo"]), "bravo": Pkg()},
        workspace_deps=["alpha"],
    )
    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["alpha", "bravo"]
    assert_first_occurrence_order(output.stderr, "bravo", "alpha")


@pytest.mark.slow
def test_path_option_publishes_only_that_package(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`--path` restricts the publish to a single package."""
    write_workspace(tmp_pixi_workspace, {"alpha": Pkg(), "bravo": Pkg()})
    _, target_dir = publish_to_dir(pixi, tmp_pixi_workspace, args=["--path", "alpha"])

    assert built_package_names(target_dir) == ["alpha"]


@pytest.mark.slow
def test_workspace_without_opted_in_packages_fails(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A workspace where no package sets `publish = true` errors out."""
    write_workspace(tmp_pixi_workspace, {})
    verify_cli_command(
        [pixi, "publish", "--target-dir", str(tmp_pixi_workspace.joinpath("dist"))],
        ExitCode.FAILURE,
        cwd=tmp_pixi_workspace,
        stderr_contains="no package in the workspace opts into publishing",
    )


@pytest.mark.slow
def test_gitignored_package_is_not_published(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Package discovery respects ignore files.

    A package that opts in with `publish = true` but sits in a gitignored
    directory is not discovered and therefore not published.
    """
    write_workspace(
        tmp_pixi_workspace,
        {"alpha": Pkg(), "vendored": Pkg()},
        workspace_deps=["alpha"],
    )
    tmp_pixi_workspace.joinpath(".gitignore").write_text("vendored/\n")

    _, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["alpha"]


@pytest.mark.slow
def test_external_path_dependency_fails(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A source dependency on a path outside the workspace root fails.

    An external source cannot be part of the published batch, so the closure
    rule refuses the publish.
    """
    workspace = _nest_workspace(tmp_pixi_workspace, "ws")
    write_workspace(workspace, {"alpha": Pkg()})
    write_standalone_package(tmp_pixi_workspace.joinpath("external", "extpkg"), "extpkg")

    manifest_path = workspace.joinpath("alpha", "pixi.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    manifest["package"].setdefault("host-dependencies", {})["extpkg"] = {
        "path": "../../external/extpkg"
    }
    manifest_path.write_text(tomli_w.dumps(manifest))

    verify_cli_command(
        [pixi, "publish", "--target-dir", str(workspace.joinpath("dist"))],
        ExitCode.FAILURE,
        cwd=workspace,
        stderr_contains="outside the workspace",
    )


@pytest.mark.slow
def test_external_git_dependency_fails(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A git source dependency fails the publish."""
    workspace = _nest_workspace(tmp_pixi_workspace, "ws")
    write_workspace(workspace, {"alpha": Pkg()})
    pkg_src = tmp_pixi_workspace.joinpath("gitsrc")
    write_standalone_package(pkg_src, "gitpkg")
    repo_url = git_test_repo(pkg_src, "gitpkg-repo", tmp_pixi_workspace.joinpath("repos"))

    manifest_path = workspace.joinpath("alpha", "pixi.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    manifest["package"].setdefault("host-dependencies", {})["gitpkg"] = {"git": repo_url}
    manifest_path.write_text(tomli_w.dumps(manifest))

    verify_cli_command(
        [pixi, "publish", "--target-dir", str(workspace.joinpath("dist"))],
        ExitCode.FAILURE,
        cwd=workspace,
        stderr_contains="outside the workspace",
    )


@pytest.mark.slow
def test_source_dependency_on_manifest_path_matches_listed_directory(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A `pkg/pixi.toml` source dependency matches the discovered `pkg` directory.

    The closure check must identify a dependency declared through the manifest
    file path with the member directory of the opted-in package.
    """
    write_workspace(tmp_pixi_workspace, {"alpha": Pkg(host=["bravo"]), "bravo": Pkg()})
    manifest_path = tmp_pixi_workspace.joinpath("alpha", "pixi.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    manifest["package"]["host-dependencies"]["bravo"] = {"path": "../bravo/pixi.toml"}
    manifest_path.write_text(tomli_w.dumps(manifest))

    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["alpha", "bravo"]
    assert "Publishing 2 workspace packages" in output.stderr


@pytest.mark.slow
def test_multi_output_member_publishes_all_outputs(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Every output of a multi-output recipe member is published."""
    write_workspace(tmp_pixi_workspace, {"multi": Pkg()}, backend="rattler-build")
    recipe = {
        "recipe": {"name": "multi", "version": "1.0.0"},
        "outputs": [
            {"package": {"name": "multi"}},
            {"package": {"name": "multi-tools"}},
        ],
    }
    tmp_pixi_workspace.joinpath("multi", "recipe.yaml").write_text(yaml.dump(recipe))

    _, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["multi", "multi-tools"]


@pytest.mark.slow
def test_second_publish_skips_existing(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Publishing into the same target twice skips the existing artifacts."""
    write_workspace(tmp_pixi_workspace, {"alpha": Pkg()})
    publish_to_dir(pixi, tmp_pixi_workspace)
    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["alpha"]
    assert "Skipping" in output.stderr, output.stderr
    assert "already exists" in output.stderr, output.stderr


@pytest.mark.slow
def test_target_channel_file_url_installs_end_to_end(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Packages published to a `file://` channel install and run elsewhere."""
    workspace = _nest_workspace(tmp_pixi_workspace, "ws")
    write_workspace(workspace, {"alpha": Pkg()})
    channel = tmp_pixi_workspace.joinpath("channel")
    verify_cli_command(
        [pixi, "publish", "--target-channel", channel.as_uri()],
        cwd=workspace,
    )
    assert channel.joinpath("noarch", "repodata.json").is_file()

    consumer = _nest_workspace(tmp_pixi_workspace, "consumer")
    consumer_manifest = {
        "workspace": {
            "channels": [channel.as_uri(), "https://prefix.dev/conda-forge"],
            "platforms": [CURRENT_PLATFORM],
        },
        "dependencies": {"alpha": "*"},
    }
    consumer.joinpath("pixi.toml").write_text(tomli_w.dumps(consumer_manifest))

    verify_cli_command(
        [pixi, "run", "alpha"],
        cwd=consumer,
        stdout_contains="hello from alpha",
    )


@pytest.mark.slow
def test_smoke_host_dep_orders_and_builds_once(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Validation smoke test: `alpha` host-depends on `bravo`.

    `bravo` must be built before `alpha` and exactly once, even though it is
    both a standalone member and a host dependency of `alpha`.
    """
    write_workspace(
        tmp_pixi_workspace,
        {
            "alpha": Pkg(host=["bravo"]),
            "bravo": Pkg(),
        },
    )
    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    built = sorted(p.name for p in target_dir.glob("*.conda"))
    assert len(built) == 2, built

    # bravo is built before alpha (dependency order). All publish output is on
    # stderr; the "Building N package(s):" list is the first place both names
    # appear, so first-occurrence order reflects build order.
    assert output.stderr.index("bravo") < output.stderr.index("alpha"), output.stderr


@pytest.mark.slow
def test_host_dep_member_is_built_only_once(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A member that is also a host dependency of another member builds once.

    `bravo` is published as a member and consumed as a host dependency of
    `alpha`. Its member build artifact must be reused when assembling
    `alpha`'s host environment instead of triggering a second build.
    """
    write_workspace(
        tmp_pixi_workspace,
        {
            "alpha": Pkg(host=["bravo"]),
            "bravo": Pkg(),
        },
    )
    output, _ = publish_to_dir(pixi, tmp_pixi_workspace)

    # The backend logs one "Running build for recipe:" line per package it
    # actually builds; cache hits produce none. Two packages, two builds.
    assert output.stderr.count("Running build for recipe") == 2, output.stderr


@pytest.mark.slow
def test_build_dep_member_is_built_only_once(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A member that is also a build dependency of another member builds once.

    Build dependencies target the build platform. Since publish targets the
    current platform by default, the build-dependency build of `bravo` must
    reuse the member build instead of running again.
    """
    write_workspace(
        tmp_pixi_workspace,
        {
            "alpha": Pkg(build=["bravo"]),
            "bravo": Pkg(),
        },
    )
    output, _ = publish_to_dir(pixi, tmp_pixi_workspace)

    assert output.stderr.count("Running build for recipe") == 2, output.stderr


@pytest.mark.slow
def test_force_overwrites_existing_artifact(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`--force` replaces an artifact that already exists at the target."""
    write_workspace(tmp_pixi_workspace, {"alpha": Pkg()})
    _, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)
    artifact = next(target_dir.glob("*.conda"))
    first_bytes = artifact.read_bytes()

    # Change the package contents without changing name/version/build string,
    # so the replacement artifact keeps the same filename. Comparing bytes
    # instead of mtime: `fs::copy` preserves timestamps on Windows and macOS.
    module = tmp_pixi_workspace.joinpath("alpha", "src", "alpha", "__init__.py")
    module.write_text('def main() -> None:\n    print("changed contents")\n')

    output, _ = publish_to_dir(pixi, tmp_pixi_workspace, args=["--force"])

    assert "already exists" not in output.stderr, output.stderr
    assert artifact.read_bytes() != first_bytes


@pytest.mark.slow
def test_no_skip_existing_fails_on_existing_artifact(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`--no-skip-existing` without `--force` refuses to overwrite."""
    write_workspace(tmp_pixi_workspace, {"alpha": Pkg()})
    _, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    verify_cli_command(
        [pixi, "publish", "--target-dir", str(target_dir), "--no-skip-existing"],
        ExitCode.FAILURE,
        cwd=tmp_pixi_workspace,
        stderr_contains="--force",
    )


@pytest.mark.slow
def test_dry_run_prints_set_without_building(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`--dry-run` prints the resolved publish set and uploads nothing."""
    write_workspace(tmp_pixi_workspace, {"alpha": Pkg(host=["bravo"]), "bravo": Pkg()})
    target_dir = tmp_pixi_workspace.joinpath("dist")
    output = verify_cli_command(
        [pixi, "publish", "--target-dir", str(target_dir), "--dry-run"],
        cwd=tmp_pixi_workspace,
    )

    assert "Would build 2 package(s)" in output.stderr
    assert_first_occurrence_order(output.stderr, "bravo", "alpha")
    assert not target_dir.exists() or built_package_names(target_dir) == []


@pytest.mark.slow
def test_force_reindexes_local_channel(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`--force` on a `file://` channel updates repodata for the new artifact.

    Overwriting the `.conda` file alone is not enough: the channel index must
    pick up the replaced artifact's checksums, otherwise consumers fail with
    checksum mismatches.
    """
    workspace = _nest_workspace(tmp_pixi_workspace, "ws")
    write_workspace(workspace, {"alpha": Pkg()})
    channel = tmp_pixi_workspace.joinpath("channel")

    verify_cli_command([pixi, "publish", "--target-channel", channel.as_uri()], cwd=workspace)
    repodata_path = channel.joinpath("noarch", "repodata.json")
    first_repodata = repodata_path.read_text()

    # Change the package contents without changing name/version/build string,
    # so the replacement artifact keeps the same filename.
    module = workspace.joinpath("alpha", "src", "alpha", "__init__.py")
    module.write_text('def main() -> None:\n    print("changed contents")\n')

    verify_cli_command(
        [pixi, "publish", "--target-channel", channel.as_uri(), "--force"],
        cwd=workspace,
    )
    second_repodata = repodata_path.read_text()

    assert second_repodata != first_repodata, "repodata must reflect the replaced artifact"


@pytest.mark.slow
def test_multi_output_sibling_dependency_uploads_in_order(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """Outputs of one recipe upload dependencies-first, not alphabetically."""
    write_workspace(tmp_pixi_workspace, {"multi": Pkg()}, backend="rattler-build")
    recipe = {
        "recipe": {"name": "multi", "version": "1.0.0"},
        "outputs": [
            {"package": {"name": "zzz-lib"}},
            {
                "package": {"name": "aaa-app"},
                "requirements": {"run": ['${{ pin_subpackage("zzz-lib", exact=True) }}']},
            },
        ],
    }
    tmp_pixi_workspace.joinpath("multi", "recipe.yaml").write_text(yaml.dump(recipe))

    output, target_dir = publish_to_dir(pixi, tmp_pixi_workspace)

    assert built_package_names(target_dir) == ["aaa-app", "zzz-lib"]
    # The upload listing reflects the build/upload order; the library that
    # `aaa-app` run-depends on must come first.
    published = output.stderr[output.stderr.index("Successfully published") :]
    assert published.index("zzz-lib") < published.index("aaa-app"), output.stderr


@pytest.mark.slow
def test_publish_from_member_directory_includes_root_package(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """The published set does not depend on the invocation directory.

    Every opted-in package - including the root `[package]` - is published
    even when the command runs inside a member directory.
    """
    write_workspace(
        tmp_pixi_workspace,
        {"rootpkg": Pkg(), "member-a": Pkg()},
        root_package="rootpkg",
        workspace_deps=["member-a"],
    )
    target_dir = tmp_pixi_workspace.joinpath("dist")
    verify_cli_command(
        [pixi, "publish", "--target-dir", str(target_dir)],
        cwd=tmp_pixi_workspace.joinpath("member-a"),
    )

    assert built_package_names(target_dir) == ["member-a", "rootpkg"]


@pytest.mark.slow
def test_deprecated_build_command_builds_only_current_package(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """`pixi build` keeps its historic single-package behavior.

    Unlike `pixi publish`, the deprecated `pixi build` command never operated
    on the whole workspace: without `--path` it builds only the package at the
    invocation directory.
    """
    write_workspace(
        tmp_pixi_workspace,
        {"rootpkg": Pkg(), "member-a": Pkg()},
        root_package="rootpkg",
    )
    target_dir = tmp_pixi_workspace.joinpath("dist")
    output = verify_cli_command(
        [pixi, "build", "--output-dir", str(target_dir)],
        cwd=tmp_pixi_workspace,
    )

    assert built_package_names(target_dir) == ["rootpkg"]
    # The suggested replacement must carry `--path`; a bare `pixi publish`
    # would publish the whole workspace instead.
    assert "--path" in output.stderr
