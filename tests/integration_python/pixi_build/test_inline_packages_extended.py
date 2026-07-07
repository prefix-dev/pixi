"""Extended integration tests for inline package definitions.

This suite exercises inline package definitions end to end: full builds
through the local rattler-build backend, workspace pool inheritance, nesting
with relative path anchoring, transitive conflict rules, seed-first override
at install time, and lock-file stability across re-verification.

Parse acceptance/rejection rules are covered by unit tests in the
`pixi_manifest` crate; only behaviour that needs the real build pipeline
lives here.

Run it with::

    pixi run test-specific-test-debug test_inline_packages_extended

Most tests run `pixi install` against recipes without any dependencies, so
they only need the workspace-built backends from
`PIXI_BUILD_BACKEND_OVERRIDE`; `test_extra_dependencies_inline_reaches_lock`
and `test_package_level_inline_edit_invalidates_lock` additionally resolve
packages from conda-forge.
"""

from pathlib import Path
from typing import Any

import pytest
import tomli_w
import yaml

from .common import (
    CONDA_FORGE_CHANNEL,
    CURRENT_PLATFORM,
    ExitCode,
    git_test_repo,
    verify_cli_command,
)

RATTLER_BACKEND: dict[str, str] = {"name": "pixi-build-rattler-build", "version": "*"}
BOGUS_BACKEND: dict[str, str] = {"name": "definitely-not-a-real-backend", "version": "*"}


def workspace_table(**extra: Any) -> dict[str, Any]:
    return {
        "channels": [CONDA_FORGE_CHANNEL],
        "platforms": [CURRENT_PLATFORM],
        "preview": ["pixi-build"],
        **extra,
    }


def inline_def(
    backend: dict[str, str] | None = None,
    run_dependencies: dict[str, Any] | None = None,
    **extra: Any,
) -> dict[str, Any]:
    """A minimal inline package definition table."""
    table: dict[str, Any] = {"build": {"backend": backend or RATTLER_BACKEND}, **extra}
    if run_dependencies is not None:
        table["run-dependencies"] = run_dependencies
    return table


def script_recipe(
    name: str,
    version: str = "1.0.0",
    extra_unix: list[str] | None = None,
    extra_win: list[str] | None = None,
    run: list[str] | None = None,
    host: list[str] | None = None,
    build: list[str] | None = None,
) -> str:
    """A recipe.yaml that installs `bin/<name>` printing `hello from <name>`.

    `extra_unix`/`extra_win` lines run before the install step, so tests can
    assert on the build environment (e.g. that a host dependency is present).

    With the rattler-build backend the recipe's `requirements` declare the
    dependencies; the manifest's `[package.*-dependencies]` tables only map
    dependency names to source locations (and inline definitions). `run`,
    `host` and `build` list the requirement names for the respective section.
    """
    unix = (extra_unix or []) + [
        "mkdir -p $PREFIX/bin",
        f'echo "#!/usr/bin/env bash" > $PREFIX/bin/{name}',
        f'echo "echo hello from {name}" >> $PREFIX/bin/{name}',
        f"chmod +x $PREFIX/bin/{name}",
    ]
    win = (extra_win or []) + [
        'if not exist "%PREFIX%\\bin" mkdir "%PREFIX%\\bin"',
        f"echo @echo off > %PREFIX%\\bin\\{name}.bat",
        f"echo echo hello from {name} >> %PREFIX%\\bin\\{name}.bat",
    ]
    recipe: dict[str, Any] = {
        "package": {"name": name, "version": version},
        "build": {
            "number": 0,
            "script": [{"if": "win", "then": win, "else": unix}],
        },
    }
    requirements = {
        key: value for key, value in (("build", build), ("host", host), ("run", run)) if value
    }
    if requirements:
        recipe["requirements"] = requirements
    return yaml.dump(recipe, sort_keys=False)


def write_recipe_source(directory: Path, name: str, **kwargs: Any) -> Path:
    """Write a manifest-less source package: a bare recipe.yaml."""
    directory.mkdir(parents=True, exist_ok=True)
    directory.joinpath("recipe.yaml").write_text(script_recipe(name, **kwargs))
    return directory


def write_manifest(path: Path, manifest: dict[str, Any]) -> Path:
    path.write_text(tomli_w.dumps(manifest))
    return path


def load_workspace(
    pixi: Path,
    manifest: Path,
    expected_exit_code: ExitCode = ExitCode.SUCCESS,
    **kwargs: Any,
) -> None:
    """Loading + converting the manifest is enough for parse-level checks."""
    verify_cli_command(
        [pixi, "workspace", "environment", "list", "--manifest-path", manifest],
        expected_exit_code=expected_exit_code,
        **kwargs,
    )


def pixi_install(pixi: Path, manifest: Path, *args: str, **kwargs: Any) -> None:
    verify_cli_command([pixi, "install", "-v", "--manifest-path", manifest, *args], **kwargs)


def pixi_run(pixi: Path, manifest: Path, task: str, **kwargs: Any) -> None:
    verify_cli_command([pixi, "run", "--manifest-path", manifest, task], **kwargs)


# ---------------------------------------------------------------------------
# Parse-level tests
#
# Parse acceptance/rejection rules (which tables allow inline definitions,
# forbidden fields, duplicates, pool inheritance, preview gating) are covered
# by unit tests in `pixi_manifest`. Only behaviour that needs the real CLI
# stays here.
# ---------------------------------------------------------------------------


def package_consumer_manifest(package_tables: dict[str, Any]) -> dict[str, Any]:
    """A workspace whose own `[package]` carries `package_tables` and depends
    on itself so the package participates in the default environment."""
    return {
        "workspace": workspace_table(name="consumer", version="0.1.0"),
        "dependencies": {"consumer": {"path": "."}},
        "package": {"build": {"backend": RATTLER_BACKEND}, **package_tables},
    }


def test_accepted_in_pyproject_package_tables(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Inline definitions in `[tool.pixi.package.run-dependencies]` of a
    pyproject.toml manifest parse like their pixi.toml counterpart."""
    write_recipe_source(tmp_pixi_workspace / "pkg", "tool-c")
    manifest = tmp_pixi_workspace / "pyproject.toml"
    manifest.write_text(
        tomli_w.dumps(
            {
                "project": {"name": "consumer", "version": "0.1.0"},
                "tool": {
                    "pixi": {
                        "workspace": workspace_table(),
                        "dependencies": {"consumer": {"path": "."}},
                        "package": {
                            "build": {"backend": RATTLER_BACKEND},
                            "run-dependencies": {
                                "tool-c": {"path": "pkg", "package": inline_def()}
                            },
                        },
                    }
                },
            }
        )
    )
    load_workspace(pixi, manifest)


# ---------------------------------------------------------------------------
# Build-level tests: happy paths
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_e2e_workspace_dependency_inline_builds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A manifest-less path source described inline in `[dependencies]` builds
    and its binary runs."""
    write_recipe_source(tmp_pixi_workspace / "pkg", "tool-c")
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"tool-c": {"path": "pkg", "package": inline_def()}},
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_e2e_recipe_file_path_inline_builds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A path source pointing directly at a recipe.yaml file (not a directory)
    builds with an inline definition."""
    write_recipe_source(tmp_pixi_workspace / "pkg", "tool-c")
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"tool-c": {"path": "pkg/recipe.yaml", "package": inline_def()}},
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_e2e_git_source_inline_builds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A manifest-less git source described inline builds and runs."""
    source = write_recipe_source(tmp_pixi_workspace / "src", "tool-c")
    repo_url = git_test_repo(source, "tool-c-repo", tmp_pixi_workspace)
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"tool-c": {"git": repo_url, "package": inline_def()}},
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_e2e_target_specific_inline_builds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """An inline definition under `[target.<platform>.dependencies]` builds."""
    write_recipe_source(tmp_pixi_workspace / "pkg", "tool-c")
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "target": {
                CURRENT_PLATFORM: {
                    "dependencies": {"tool-c": {"path": "pkg", "package": inline_def()}}
                }
            },
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_e2e_nested_inline_definitions_build(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Inline definitions nest: lib-b is declared inline, and its definition
    declares tool-c inline in its own run-dependencies.

    The path of tool-c (`../c_pkg`) is written inside lib-b's definition, so
    it must resolve relative to lib-b's source directory (`sub/b_pkg`), not
    relative to the consuming manifest. Both packages live under `sub/` while
    the consumer manifest is at the workspace root, so wrong anchoring cannot
    accidentally resolve."""
    write_recipe_source(tmp_pixi_workspace / "sub" / "b_pkg", "lib-b", run=["tool-c"])
    write_recipe_source(tmp_pixi_workspace / "sub" / "c_pkg", "tool-c")
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {
                "lib-b": {
                    "path": "sub/b_pkg",
                    "package": inline_def(
                        run_dependencies={"tool-c": {"path": "../c_pkg", "package": inline_def()}}
                    ),
                }
            },
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "lib-b", stdout_contains="hello from lib-b")
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_e2e_package_host_dependency_inline_builds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """An inline definition in `[package.host-dependencies]` is built and
    installed into the host environment of the consuming package's build.

    The consumer's build script fails unless the host prefix contains the
    binary installed by tool-c, so a passing install proves the host dep was
    built from its inline definition and injected."""
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    tmp_pixi_workspace.joinpath("recipe.yaml").write_text(
        script_recipe(
            "consumer",
            host=["tool-c"],
            extra_unix=['test -x "$PREFIX/bin/tool-c"'],
            extra_win=['if not exist "%PREFIX%\\bin\\tool-c.bat" exit 1'],
        )
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        package_consumer_manifest(
            {"host-dependencies": {"tool-c": {"path": "c_pkg", "package": inline_def()}}}
        ),
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "consumer", stdout_contains="hello from consumer")


@pytest.mark.slow
def test_e2e_package_build_dependency_inline_builds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """An inline definition in `[package.build-dependencies]` lands in the
    build environment of the consuming package's build."""
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    tmp_pixi_workspace.joinpath("recipe.yaml").write_text(
        script_recipe(
            "consumer",
            build=["tool-c"],
            extra_unix=['test -x "$BUILD_PREFIX/bin/tool-c"'],
            extra_win=['if not exist "%BUILD_PREFIX%\\bin\\tool-c.bat" exit 1'],
        )
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        package_consumer_manifest(
            {"build-dependencies": {"tool-c": {"path": "c_pkg", "package": inline_def()}}}
        ),
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "consumer", stdout_contains="hello from consumer")


@pytest.mark.slow
def test_e2e_pool_inheritance_builds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A pool entry carrying an inline definition, inherited with
    `{ workspace = true }` in `[package.run-dependencies]`, builds end to end."""
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    tmp_pixi_workspace.joinpath("recipe.yaml").write_text(script_recipe("consumer", run=["tool-c"]))
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(
                name="consumer",
                version="0.1.0",
                dependencies={"tool-c": {"path": "c_pkg", "package": inline_def()}},
            ),
            "dependencies": {"consumer": {"path": "."}},
            "package": {
                "build": {"backend": RATTLER_BACKEND},
                "run-dependencies": {"tool-c": {"workspace": True}},
            },
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_e2e_conditional_package_dependency_builds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """An inline definition inside an `if(...)` conditional package dependency
    table builds when the condition matches the platform."""
    condition = "if(win)" if CURRENT_PLATFORM.startswith("win") else "if(unix)"
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    tmp_pixi_workspace.joinpath("recipe.yaml").write_text(script_recipe("consumer", run=["tool-c"]))
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        package_consumer_manifest(
            {
                "run-dependencies": {
                    condition: {"tool-c": {"path": "c_pkg", "package": inline_def()}}
                }
            }
        ),
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_extra_dependencies_inline_reaches_lock(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """An inline definition in `[package.extra-dependencies.<group>]` resolves
    into the lock file when the consumer requests the extra.

    The python backend is used because the rattler-build backend derives its
    requirements from the recipe and does not report manifest extras."""
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    tmp_pixi_workspace.joinpath("pyproject.toml").write_text(
        tomli_w.dumps(
            {
                "project": {"name": "consumer", "version": "0.1.0"},
                "build-system": {
                    "requires": ["hatchling"],
                    "build-backend": "hatchling.build",
                },
            }
        )
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(name="consumer", version="0.1.0"),
            "dependencies": {"consumer": {"path": ".", "extras": ["dev"]}},
            "package": {
                "build": {"backend": {"name": "pixi-build-python", "version": "*"}},
                "host-dependencies": {"hatchling": "*"},
                "extra-dependencies": {
                    "dev": {"tool-c": {"path": "c_pkg", "package": inline_def()}}
                },
            },
        },
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    lock_text = tmp_pixi_workspace.joinpath("pixi.lock").read_text()
    assert "tool-c" in lock_text, (
        "the inline-defined extra dependency never resolved into the lock file"
    )


# ---------------------------------------------------------------------------
# Build-level tests: conflict semantics
# ---------------------------------------------------------------------------


def write_declaring_package(
    directory: Path,
    name: str,
    dependency_tables: dict[str, Any],
) -> Path:
    """An on-disk source package (rattler-build recipe plus pixi.toml with a
    `[package]` section) that declares `dependency_tables`.

    The recipe's `requirements` mirror the dependency tables so the backend
    reports the dependencies; the manifest tables carry the source locations
    and inline definitions."""
    table_to_section = {
        "run-dependencies": "run",
        "host-dependencies": "host",
        "build-dependencies": "build",
    }
    requirements: dict[str, list[str]] = {}
    for table, entries in dependency_tables.items():
        section = table_to_section.get(table)
        if section is not None:
            requirements[section] = list(entries)
    directory.mkdir(parents=True, exist_ok=True)
    directory.joinpath("recipe.yaml").write_text(
        script_recipe(
            name,
            run=requirements.get("run"),
            host=requirements.get("host"),
            build=requirements.get("build"),
        )
    )
    write_manifest(
        directory / "pixi.toml",
        {
            "package": {
                "name": name,
                "version": "1.0.0",
                "build": {"backend": RATTLER_BACKEND},
                **dependency_tables,
            }
        },
    )
    return directory


@pytest.mark.slow
def test_transitive_conflicting_definitions_error(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Two packages that disagree about the inline definition for the same
    `(package, source location)` fail the solve, naming both parents."""
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    write_declaring_package(
        tmp_pixi_workspace / "a_pkg",
        "pkg-a",
        {
            "run-dependencies": {
                "tool-c": {"path": "../c_pkg", "package": inline_def(version="1.0.0")}
            }
        },
    )
    write_declaring_package(
        tmp_pixi_workspace / "b_pkg",
        "pkg-b",
        {
            "run-dependencies": {
                "tool-c": {"path": "../c_pkg", "package": inline_def(version="2.0.0")}
            }
        },
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {
                "pkg-a": {"path": "a_pkg"},
                "pkg-b": {"path": "b_pkg"},
            },
        },
    )
    pixi_install(
        pixi,
        manifest,
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains=["conflicting inline definitions", "pkg-a", "pkg-b"],
    )


@pytest.mark.slow
def test_transitive_same_definition_ok(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Two packages declaring the identical definition for the same
    `(package, source location)` deduplicate instead of conflicting."""
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    tables = {
        "run-dependencies": {"tool-c": {"path": "../c_pkg", "package": inline_def(version="1.0.0")}}
    }
    write_declaring_package(tmp_pixi_workspace / "a_pkg", "pkg-a", tables)
    write_declaring_package(tmp_pixi_workspace / "b_pkg", "pkg-b", tables)
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {
                "pkg-a": {"path": "a_pkg"},
                "pkg-b": {"path": "b_pkg"},
            },
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_transitive_definition_vs_plain_conflict(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A package declaring a definition and another declaring the same
    dependency plain (relying on the on-disk manifest) is ambiguous and fails."""
    c_pkg = write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    write_manifest(
        c_pkg / "pixi.toml",
        {
            "package": {
                "name": "tool-c",
                "version": "1.0.0",
                "build": {"backend": RATTLER_BACKEND},
            }
        },
    )
    write_declaring_package(
        tmp_pixi_workspace / "a_pkg",
        "pkg-a",
        {"run-dependencies": {"tool-c": {"path": "../c_pkg", "package": inline_def()}}},
    )
    write_declaring_package(
        tmp_pixi_workspace / "b_pkg",
        "pkg-b",
        {"run-dependencies": {"tool-c": {"path": "../c_pkg"}}},
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {
                "pkg-a": {"path": "a_pkg"},
                "pkg-b": {"path": "b_pkg"},
            },
        },
    )
    pixi_install(
        pixi,
        manifest,
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="conflicting inline definitions",
    )


@pytest.mark.slow
def test_direct_dependency_overrides_transitive_definition(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A direct dependency of the environment overrides package-level
    definitions: pkg-a's bogus definition for tool-c must lose against the
    workspace's own plain declaration of tool-c (on-disk manifest)."""
    c_pkg = write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    write_manifest(
        c_pkg / "pixi.toml",
        {
            "package": {
                "name": "tool-c",
                "version": "1.0.0",
                "build": {"backend": RATTLER_BACKEND},
            }
        },
    )
    write_declaring_package(
        tmp_pixi_workspace / "a_pkg",
        "pkg-a",
        {
            "run-dependencies": {
                "tool-c": {"path": "../c_pkg", "package": inline_def(backend=BOGUS_BACKEND)}
            }
        },
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {
                "pkg-a": {"path": "a_pkg"},
                "tool-c": {"path": "c_pkg"},
            },
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "tool-c", stdout_contains="hello from tool-c")


@pytest.mark.slow
def test_two_environments_resolve_definitions_independently(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """Each environment resolves its own inline definition for the same
    `(package, source location)`: two features carrying different definitions
    in different environments must not be treated as a conflict (the
    transitive conflict rule applies within one environment's walk)."""
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "feature": {
                "one": {
                    "dependencies": {
                        "tool-c": {"path": "c_pkg", "package": inline_def(version="1.0.0")}
                    }
                },
                "two": {
                    "dependencies": {
                        "tool-c": {"path": "c_pkg", "package": inline_def(version="2.0.0")}
                    }
                },
            },
            "environments": {"env-one": ["one"], "env-two": ["two"]},
        },
    )
    pixi_install(pixi, manifest, "--environment", "env-one")
    pixi_install(pixi, manifest, "--environment", "env-two")
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "env-one", "tool-c"],
        stdout_contains="hello from tool-c",
    )


# ---------------------------------------------------------------------------
# Build-level tests: lock-file behaviour
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_lock_stable_with_transitive_inline_definition(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A lock file produced with a package-level (transitive) inline
    definition must stay satisfied on the next runs; re-verification must not
    flip-flop into a re-lock loop."""
    write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    write_declaring_package(
        tmp_pixi_workspace / "a_pkg",
        "pkg-a",
        {"run-dependencies": {"tool-c": {"path": "../c_pkg", "package": inline_def()}}},
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"pkg-a": {"path": "a_pkg"}},
        },
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )
    pixi_install(pixi, manifest)
    verify_cli_command([pixi, "lock", "--check", "--manifest-path", manifest])


@pytest.mark.slow
def test_lock_stable_with_inline_definition_behind_plain_dependency(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A definition declared two hops down (seed -> plain path dependency ->
    inline-defined package) must reach its package during re-verification.

    The declarer (pkg-b) is itself a plain dependency that never receives a
    definition; verification must still query it before the package it
    declares (tool-c), instead of querying both without a definition once no
    progress is made. tool-c's on-disk manifest names a bogus backend, so a
    query that drops the definition fails observably."""
    c_pkg = write_recipe_source(tmp_pixi_workspace / "c_pkg", "tool-c")
    c_pkg.joinpath("pixi.toml").write_text(
        tomli_w.dumps(
            {
                "package": {
                    "name": "tool-c",
                    "version": "0.1.0",
                    "build": {"backend": BOGUS_BACKEND},
                }
            }
        )
    )
    write_declaring_package(
        tmp_pixi_workspace / "b_pkg",
        "pkg-b",
        {"run-dependencies": {"tool-c": {"path": "../c_pkg", "package": inline_def()}}},
    )
    write_declaring_package(
        tmp_pixi_workspace / "a_pkg",
        "pkg-a",
        {"run-dependencies": {"pkg-b": {"path": "../b_pkg"}}},
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"pkg-a": {"path": "a_pkg"}},
        },
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )


@pytest.mark.slow
def test_solve_group_inline_definition_lock_stable(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A source dependency declared with an inline definition by one
    solve-group member must verify against the group-level definition in a
    sibling environment that only reaches it transitively.

    The solve seeds at solve-group scope, so the group-level definition is
    folded into the locked identifier hash even for the sibling. Verification
    classifying seeds per environment would check the record against a
    package-level declarer (none here) instead and re-lock forever."""
    source = write_recipe_source(tmp_pixi_workspace / "c_src", "tool-c")
    repo_url = git_test_repo(source, "tool-c-repo", tmp_pixi_workspace)
    write_declaring_package(
        tmp_pixi_workspace / "a_pkg",
        "pkg-a",
        {"run-dependencies": {"tool-c": {"git": repo_url}}},
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"pkg-a": {"path": "a_pkg"}},
            "feature": {
                "tools": {"dependencies": {"tool-c": {"git": repo_url, "package": inline_def()}}}
            },
            "environments": {
                "default": {"features": [], "solve-group": "grp"},
                "tools": {"features": ["tools"], "solve-group": "grp"},
            },
        },
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )


@pytest.mark.slow
def test_package_level_inline_edit_invalidates_lock(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Editing an inline definition declared in a *source dependency's* own
    manifest (a package-level declarer) must invalidate the lock file.

    tool-c uses the python backend, whose reported metadata derives from the
    inline definition, so adding a run dependency there is an observable
    change. Re-verification must pick up the declarer's edited definition
    instead of trusting the stale locked record."""
    c_pkg = tmp_pixi_workspace / "c_pkg"
    c_pkg.mkdir(parents=True, exist_ok=True)
    c_pkg.joinpath("pyproject.toml").write_text(
        tomli_w.dumps(
            {
                "project": {"name": "tool-c", "version": "0.1.0"},
                "build-system": {
                    "requires": ["setuptools"],
                    "build-backend": "setuptools.build_meta",
                },
            }
        )
    )
    a_pkg = tmp_pixi_workspace / "a_pkg"

    def declare(run_dependencies: dict[str, str] | None) -> None:
        package: dict[str, Any] = {
            "version": "0.1.0",
            "build": {"backend": {"name": "pixi-build-python", "version": "*"}},
            "host-dependencies": {"setuptools": "*"},
        }
        if run_dependencies:
            package["run-dependencies"] = run_dependencies
        write_declaring_package(
            a_pkg,
            "pkg-a",
            {"run-dependencies": {"tool-c": {"path": "../c_pkg", "package": package}}},
        )

    declare(None)
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"pkg-a": {"path": "a_pkg"}},
        },
    )
    lock_file = tmp_pixi_workspace / "pixi.lock"
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    assert "rich" not in lock_file.read_text()

    declare({"rich": "*"})
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    assert "rich" in lock_file.read_text(), (
        "editing the package-level inline definition did not invalidate the lock file"
    )
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )


@pytest.mark.slow
def test_same_name_other_location_definition_not_misapplied(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """An inline definition declared for a package name at one source location
    must not be applied to a record of the same name at another location.

    The workspace's own `[package.host-dependencies]` declares tool-c at a
    path location with an inline definition; the runtime environment contains
    a different tool-c from a git location, reached through pkg-a without a
    definition. Applying the path declaration's definition to the git record
    changes its identity hash and re-locks forever."""
    source = write_recipe_source(tmp_pixi_workspace / "c_src", "tool-c")
    repo_url = git_test_repo(source, "tool-c-repo", tmp_pixi_workspace)
    write_recipe_source(tmp_pixi_workspace / "c_decl", "tool-c")
    write_declaring_package(
        tmp_pixi_workspace / "a_pkg",
        "pkg-a",
        {"run-dependencies": {"tool-c": {"git": repo_url}}},
    )
    tmp_pixi_workspace.joinpath("recipe.yaml").write_text(
        script_recipe("consumer", host=["tool-c"])
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(name="consumer", version="0.1.0"),
            "dependencies": {"consumer": {"path": "."}, "pkg-a": {"path": "a_pkg"}},
            "package": {
                "build": {"backend": RATTLER_BACKEND},
                "host-dependencies": {"tool-c": {"path": "c_decl", "package": inline_def()}},
            },
        },
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )


@pytest.mark.slow
def test_git_transitive_inline_lock_stable(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A git source whose inline definition is declared by a transitive parent
    must not re-lock on every run.

    The definition's content hash is folded into the locked identifier hash.
    Verification must reproduce that hash from the parent's definition instead
    of comparing against the (empty) environment-level definitions, which
    would flag the record as changed forever."""
    source = write_recipe_source(tmp_pixi_workspace / "c_src", "tool-c")
    repo_url = git_test_repo(source, "tool-c-repo", tmp_pixi_workspace)
    write_declaring_package(
        tmp_pixi_workspace / "a_pkg",
        "pkg-a",
        {"run-dependencies": {"tool-c": {"git": repo_url, "package": inline_def()}}},
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"pkg-a": {"path": "a_pkg"}},
        },
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )


@pytest.mark.slow
def test_package_level_inline_removal_invalidates_lock(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """Removing the inline definition from a declaring parent's manifest must
    invalidate the lock file.

    The definition's content hash is folded into the locked identifier hash
    of the (immutable) git record. Once the only declarer stops providing a
    definition, recomputing the hash without one must flag the record as
    changed instead of silently trusting the stale lock."""
    source = write_recipe_source(tmp_pixi_workspace / "c_src", "tool-c")
    repo_url = git_test_repo(source, "tool-c-repo", tmp_pixi_workspace)
    a_pkg = tmp_pixi_workspace / "a_pkg"

    def declare(with_definition: bool) -> None:
        spec: dict[str, Any] = {"git": repo_url}
        if with_definition:
            spec["package"] = inline_def()
        write_declaring_package(a_pkg, "pkg-a", {"run-dependencies": {"tool-c": spec}})

    declare(with_definition=True)
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"pkg-a": {"path": "a_pkg"}},
        },
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])

    declare(with_definition=False)
    # The record's version and build do not change, so the visible diff is
    # empty; the re-written identifier hash is what marks the update.
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="Updated lock file",
    )
    # Re-locking without the definition converges again.
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )


@pytest.mark.slow
def test_git_package_table_inline_lock_stable(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A git source whose inline definition lives in the workspace's own
    `[package.run-dependencies]` must not re-lock on every run."""
    source = write_recipe_source(tmp_pixi_workspace / "c_src", "tool-c")
    repo_url = git_test_repo(source, "tool-c-repo", tmp_pixi_workspace)
    tmp_pixi_workspace.joinpath("recipe.yaml").write_text(script_recipe("consumer", run=["tool-c"]))
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        package_consumer_manifest(
            {"run-dependencies": {"tool-c": {"git": repo_url, "package": inline_def()}}}
        ),
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )


@pytest.mark.slow
def test_nested_definition_harvested_regardless_of_record_order(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A definition nested inside another record's inline manifest must be
    harvested at install time even when the inline-defined record is
    processed before its declarer.

    zzz-parent declares aaa-mid inline, and that definition declares bbb-leaf
    inline. aaa-mid sorts before zzz-parent, so a harvest that commits
    aaa-mid on first discovery reads its decoy on-disk manifest (bogus
    backend, no nested definition) and never learns bbb-leaf's definition;
    bbb-leaf's own decoy manifest then fails the build."""

    def write_decoy_manifest(directory: Path, name: str) -> None:
        directory.joinpath("pixi.toml").write_text(
            tomli_w.dumps(
                {
                    "package": {
                        "name": name,
                        "version": "0.1.0",
                        "build": {"backend": BOGUS_BACKEND},
                    }
                }
            )
        )

    mid = write_recipe_source(tmp_pixi_workspace / "mid_pkg", "aaa-mid", run=["bbb-leaf"])
    write_decoy_manifest(mid, "aaa-mid")
    leaf = write_recipe_source(tmp_pixi_workspace / "leaf_pkg", "bbb-leaf")
    write_decoy_manifest(leaf, "bbb-leaf")
    write_declaring_package(
        tmp_pixi_workspace / "parent_pkg",
        "zzz-parent",
        {
            "run-dependencies": {
                "aaa-mid": {
                    "path": "../mid_pkg",
                    "package": inline_def(
                        run_dependencies={
                            "bbb-leaf": {"path": "../leaf_pkg", "package": inline_def()}
                        }
                    ),
                }
            }
        },
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"zzz-parent": {"path": "parent_pkg"}},
        },
    )
    pixi_install(pixi, manifest)
    pixi_run(pixi, manifest, "bbb-leaf", stdout_contains="hello from bbb-leaf")


@pytest.mark.slow
def test_nested_env_keeps_seed_choice_over_sibling_definition(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A sibling's inline definition must not override the nested solve's
    seed choice when a build environment is installed.

    The workspace package declares build dependencies tool-x (plain, with an
    on-disk recipe) and pkg-y, whose own manifest declares tool-x with a
    bogus-backend inline definition. The nested solve seeds tool-x with the
    package's plain declaration (seed-first), so the build must use the
    on-disk recipe; picking up pkg-y's definition instead fails on the bogus
    backend."""
    write_recipe_source(tmp_pixi_workspace / "x_pkg", "tool-x")
    write_declaring_package(
        tmp_pixi_workspace / "y_pkg",
        "pkg-y",
        {
            "run-dependencies": {
                "tool-x": {"path": "../x_pkg", "package": inline_def(BOGUS_BACKEND)}
            }
        },
    )
    tmp_pixi_workspace.joinpath("recipe.yaml").write_text(
        script_recipe("consumer", build=["tool-x", "pkg-y"])
    )
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(name="consumer", version="0.1.0"),
            "dependencies": {"consumer": {"path": "."}},
            "package": {
                "build": {"backend": RATTLER_BACKEND},
                "build-dependencies": {
                    "tool-x": {"path": "x_pkg"},
                    "pkg-y": {"path": "y_pkg"},
                },
            },
        },
    )
    pixi_install(pixi, manifest)


# ---------------------------------------------------------------------------
# Build-level tests: diagnostics
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_recipe_name_mismatch_fails_comprehensibly(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """The dependency key names the package; a recipe producing a different
    name must fail with an error rather than silently misresolving."""
    write_recipe_source(tmp_pixi_workspace / "pkg", "other-name")
    manifest = write_manifest(
        tmp_pixi_workspace / "pixi.toml",
        {
            "workspace": workspace_table(),
            "dependencies": {"tool-c": {"path": "pkg", "package": inline_def()}},
        },
    )
    pixi_install(
        pixi,
        manifest,
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="tool-c",
    )
