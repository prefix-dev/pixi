import json
import os
from pathlib import Path
from typing import Any

import pytest
import tomli
import tomli_w
import yaml
from rattler.package_streaming import extract

from .common import CURRENT_PLATFORM, ExitCode, copytree_with_local_backend, verify_cli_command

pytestmark = pytest.mark.slow

PACKAGE_NAME = "mojo-accelerator-test"
CHANNELS = ["https://conda.modular.com/max", "https://prefix.dev/conda-forge"]
CPU_ENV = {"CONDA_OVERRIDE_AMDGPU": "", "CONDA_OVERRIDE_AMDGPU_ARCH": ""}


@pytest.fixture(autouse=True)
def local_mojo_backend(pixi: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    backend = pixi.parent.joinpath("pixi-build-mojo").resolve()
    assert backend.is_file(), f"Build the local Mojo backend first: {backend}"
    overrides = os.environ.get("PIXI_BUILD_BACKEND_OVERRIDE", "")
    entries = [
        entry
        for entry in overrides.split(",")
        if entry and not entry.startswith("pixi-build-mojo=")
    ]
    entries.append(f"pixi-build-mojo={backend}")
    monkeypatch.setenv("PIXI_BUILD_BACKEND_OVERRIDE", ",".join(entries))


@pytest.fixture
def mojo_workspace(tmp_pixi_workspace: Path, build_data: Path) -> Path:
    copytree_with_local_backend(
        build_data.joinpath("mojo-accelerator"), tmp_pixi_workspace, dirs_exist_ok=True
    )
    return tmp_pixi_workspace


def _set_variants(workspace: Path, variants: list[str]) -> None:
    path = workspace.joinpath("pixi.toml")
    manifest = tomli.loads(path.read_text())
    manifest["workspace"]["build-variants"]["mojo_accelerator"] = variants
    path.write_text(tomli_w.dumps(manifest))


def _package_index(archive: Path, destination: Path) -> dict[str, Any]:
    extract(archive, destination)
    return json.loads(destination.joinpath("info", "index.json").read_text())


def _runtime_pin(index: dict[str, Any], name: str, version: str) -> str:
    requirements = [dependency.split() for dependency in index["depends"]]
    exact = [
        parts
        for parts in requirements
        if parts[0] == name
        and len(parts) == 3
        and parts[1].lstrip("=") == version
        and not any(character in parts[2] for character in "*?[]")
    ]
    assert len(exact) == 1, index["depends"]
    return exact[0][2]


def _write_consumer(workspace: Path, channel: Path, platforms: list[dict[str, Any]]) -> None:
    workspace.mkdir()
    manifest = {
        "workspace": {
            "channels": [channel.as_uri(), *CHANNELS],
            "platforms": platforms,
        },
        "dependencies": {PACKAGE_NAME: "*"},
    }
    workspace.joinpath("pixi.toml").write_text(tomli_w.dumps(manifest))


@pytest.mark.skipif(
    CURRENT_PLATFORM not in ("linux-64", "osx-arm64"), reason="Needs the Mojo compiler"
)
def test_mojo_binary_installs_compiler_runtime(pixi: Path, tmp_pixi_workspace: Path) -> None:
    workspace = tmp_pixi_workspace
    workspace.joinpath("main.mojo").write_text('def main():\n    print("hello from Mojo")\n')
    platforms = [{"platform": CURRENT_PLATFORM}]
    build_variants = {}
    build_config: dict[str, Any] = {"bins": [{"name": PACKAGE_NAME, "path": "main.mojo"}]}
    if CURRENT_PLATFORM == "linux-64":
        platforms[0]["glibc"] = "2.34"
        build_variants["c_stdlib_version"] = ["2.34"]
        linker = workspace.joinpath("link-driver.sh")
        linker.write_text('#!/bin/sh\nexec "$CC" -Wl,-rpath-link,"$PREFIX/lib" "$@"\n')
        linker.chmod(0o755)
        build_config.update(
            {
                "compilers": ["c", "cxx"],
                "env": {"MODULAR_MOJO_MAX_LINKER_DRIVER": str(linker)},
            }
        )
    manifest = {
        "workspace": {
            "channels": CHANNELS,
            "platforms": platforms,
            "preview": ["pixi-build"],
            "build-variants": build_variants,
        },
        "package": {
            "name": PACKAGE_NAME,
            "version": "1.0.0",
            "build": {
                "backend": {"name": "pixi-build-mojo", "version": "*"},
                "config": build_config,
            },
            "build-dependencies": {"mojo-compiler": "==1.2.0"},
        },
    }
    workspace.joinpath("pixi.toml").write_text(tomli_w.dumps(manifest))
    channel = workspace.joinpath("channel")
    verify_cli_command(
        [pixi, "publish", "--target-channel", channel.as_uri()],
        cwd=workspace,
    )
    [archive] = channel.joinpath(CURRENT_PLATFORM).glob("*.conda")
    index = _package_index(archive, workspace.joinpath("extracted"))
    compiler_build = _runtime_pin(index, "mojo-compiler", "1.2.0")

    consumer = workspace.joinpath("consumer")
    _write_consumer(consumer, channel, platforms)
    output = verify_cli_command([pixi, "run", PACKAGE_NAME], cwd=consumer)
    assert output.stdout == "hello from Mojo\n"
    [compiler_record] = consumer.joinpath(".pixi", "envs", "default", "conda-meta").glob(
        "mojo-compiler-*.json"
    )
    compiler = json.loads(compiler_record.read_text())
    assert compiler["version"] == "1.2.0"
    assert compiler["build"] == compiler_build


@pytest.mark.skipif(
    CURRENT_PLATFORM not in ("linux-64", "osx-arm64"), reason="Needs the Mojo compiler"
)
@pytest.mark.parametrize("compiler_environment", ["build", "host", "transitive-build"])
def test_mojo_library_pins_compiler(
    pixi: Path, tmp_pixi_workspace: Path, compiler_environment: str
) -> None:
    workspace = tmp_pixi_workspace
    source = workspace.joinpath("src")
    source.mkdir()
    source.joinpath("__init__.mojo").write_text("def answer() -> Int:\n    return 42\n")
    compiler_section = (
        "host-dependencies" if compiler_environment == "host" else "build-dependencies"
    )
    manifest = {
        "workspace": {
            "channels": CHANNELS,
            "platforms": [CURRENT_PLATFORM],
            "preview": ["pixi-build"],
        },
        "package": {
            "name": PACKAGE_NAME,
            "version": "1.0.0",
            "build": {
                "backend": {"name": "pixi-build-mojo", "version": "*"},
                "config": {"pkg": {"name": "probe", "path": "src"}},
            },
            compiler_section: {"mojo-compiler": "==1.2.0"},
        },
    }
    if compiler_environment == "build":
        manifest["package"]["host-dependencies"] = {"mojo-compiler": "*"}
    channel = workspace.joinpath("channel")
    compiler_version = "1.2.0"
    if compiler_environment == "transitive-build":
        provider = workspace.joinpath("provider")
        provider.mkdir()
        manifest["package"]["name"] = "mojo-compiler-provider"
        manifest["package"]["build"]["config"]["pkg"]["path"] = str(source)
        provider.joinpath("pixi.toml").write_text(tomli_w.dumps(manifest))
        verify_cli_command(
            [pixi, "publish", "--target-channel", channel.as_uri()],
            cwd=provider,
        )
        manifest["package"]["name"] = PACKAGE_NAME
        manifest["package"]["build"]["config"]["pkg"]["path"] = "src"
        manifest["workspace"]["channels"] = [channel.as_uri(), *CHANNELS]
        manifest["package"]["build-dependencies"] = {"mojo-compiler-provider": "*"}
        compiler_version = "1.1.0"
        manifest["package"]["host-dependencies"] = {"mojo-compiler": f"=={compiler_version}"}
    workspace.joinpath("pixi.toml").write_text(tomli_w.dumps(manifest))
    verify_cli_command(
        [pixi, "publish", "--target-channel", channel.as_uri()],
        cwd=workspace,
    )
    [archive] = channel.joinpath(CURRENT_PLATFORM).glob(f"{PACKAGE_NAME}-*.conda")
    index = _package_index(archive, workspace.joinpath("extracted"))
    compiler_build = _runtime_pin(index, "mojo-compiler", compiler_version)

    consumer = workspace.joinpath("consumer")
    _write_consumer(consumer, channel, [{"platform": CURRENT_PLATFORM}])
    consumer.joinpath("main.mojo").write_text(
        "from probe import answer\n\ndef main():\n    print(answer())\n"
    )
    output = verify_cli_command([pixi, "run", "mojo", "main.mojo"], cwd=consumer)
    assert output.stdout == "42\n"
    [compiler_record] = consumer.joinpath(".pixi", "envs", "default", "conda-meta").glob(
        "mojo-compiler-*.json"
    )
    compiler = json.loads(compiler_record.read_text())
    assert compiler["version"] == compiler_version
    assert compiler["build"] == compiler_build

    manifest_path = consumer.joinpath("pixi.toml")
    consumer_manifest = tomli.loads(manifest_path.read_text())
    consumer_manifest["dependencies"]["mojo-compiler"] = f"!={compiler_version}"
    manifest_path.write_text(tomli_w.dumps(consumer_manifest))
    verify_cli_command([pixi, "lock"], ExitCode.FAILURE, cwd=consumer)


@pytest.mark.skipif(
    CURRENT_PLATFORM != "linux-64", reason="Mojo accelerator binaries need linux-64"
)
def test_mojo_variants_publish_solve_and_run(pixi: Path, mojo_workspace: Path) -> None:
    channel = mojo_workspace.joinpath("channel")
    verify_cli_command(
        [
            pixi,
            "publish",
            "--path",
            mojo_workspace.joinpath("pixi.toml"),
            "--target-channel",
            channel.as_uri(),
        ],
        cwd=mojo_workspace,
    )

    archives = sorted(channel.joinpath("linux-64").glob("*.conda"))
    assert len(archives) == 3
    packages = {}
    for position, archive in enumerate(archives):
        index = _package_index(archive, mojo_workspace.joinpath(f"extracted-{position}"))
        arch = [
            dependency for dependency in index["depends"] if dependency.startswith("__amdgpu_arch")
        ]
        variant = (
            {"__amdgpu_arch ==11.0.0": "gfx1100", "__amdgpu_arch ==11.0.3": "gfx1103"}[arch[0]]
            if arch
            else "cpu"
        )
        assert variant not in packages
        packages[variant] = (archive, index)
        assert index["name"] == PACKAGE_NAME
        assert index["subdir"] == "linux-64"
        assert index["build_number"] == 0
        assert "__glibc >=2.34" in index["depends"]
        assert "_x86_64-microarch-level >=3" in index["depends"]
        _runtime_pin(index, "mojo-compiler", "1.2.0")
        _runtime_pin(index, "max-core", "26.7.0")

    assert set(packages) == {"cpu", "gfx1100", "gfx1103"}
    cpu_archive, cpu_index = packages["cpu"]
    other_amd_archive, other_amd_index = packages["gfx1100"]
    amd_archive, amd_index = packages["gfx1103"]
    assert len({cpu_index["build"], other_amd_index["build"], amd_index["build"]}) == 3
    assert cpu_index["track_features"] == f"{PACKAGE_NAME}-p-0"
    for index in [other_amd_index, amd_index]:
        assert not index.get("track_features")
        assert {"__amdgpu", "hip-runtime-amd >=6.3.3"} <= set(index["depends"])
    assert not any(
        dependency.split()[0] in {"__amdgpu", "__amdgpu_arch", "hip-runtime-amd"}
        for dependency in cpu_index["depends"]
    )
    for provider, version in [("mojo-compiler", "1.2.0"), ("max-core", "26.7.0")]:
        assert _runtime_pin(cpu_index, provider, version) == _runtime_pin(
            amd_index, provider, version
        )

    # Switching one source workspace must never reuse the other variant's artifact.
    source_path = mojo_workspace.joinpath("main.mojo")
    source = source_path.read_text()
    for position, variant in enumerate(["cpu", "gfx1103", "cpu"]):
        _set_variants(mojo_workspace, [variant])
        source_path.write_text(
            source.replace(
                "def main() raises:",
                f'def main() raises:\n    comptime assert accelerator == "{variant}", "isolated rebuild {position}"',
            )
        )
        destination = mojo_workspace.joinpath(f"isolated-{position}")
        verify_cli_command(
            [
                pixi,
                "publish",
                "--path",
                mojo_workspace.joinpath("pixi.toml"),
                "--target-dir",
                destination,
            ],
            cwd=mojo_workspace,
        )
        outputs = list(destination.glob("*.conda"))
        assert [archive.name for archive in outputs] == [packages[variant][0].name]
        index = _package_index(outputs[0], mojo_workspace.joinpath(f"isolated-index-{position}"))
        assert index["build"] == packages[variant][1]["build"]
        assert index["depends"] == packages[variant][1]["depends"]

    consumer = mojo_workspace.joinpath("consumer")
    platforms: list[dict[str, Any]] = [
        {"name": "cpu", "platform": "linux-64", "glibc": "2.34"},
        {"name": "matching-amd", "platform": "linux-64", "glibc": "2.34", "amdgpu": "gfx1103"},
        {"name": "other-amd", "platform": "linux-64", "glibc": "2.34", "amdgpu": "gfx1100"},
        {"name": "newer-amd", "platform": "linux-64", "glibc": "2.34", "amdgpu": "gfx1150"},
        {"name": "newer-cpu", "platform": "linux-64", "glibc": "2.34", "archspec": "x86_64_v4"},
    ]
    for platform in platforms:
        platform.setdefault("archspec", "x86_64_v3")
    _write_consumer(consumer, channel, platforms)
    verify_cli_command([pixi, "lock"], cwd=consumer)
    lock = yaml.safe_load(consumer.joinpath("pixi.lock").read_text())
    selections = lock["environments"]["default"]["packages"]
    for platform, expected in [
        ("cpu", cpu_archive),
        ("matching-amd", amd_archive),
        ("other-amd", other_amd_archive),
        ("newer-amd", cpu_archive),
        ("newer-cpu", cpu_archive),
    ]:
        selected = [
            record["conda"]
            for record in selections[platform]
            if Path(record["conda"]).name.startswith(f"{PACKAGE_NAME}-")
        ]
        assert len(selected) == 1, selections[platform]
        assert Path(selected[0]).name == expected.name

    verify_cli_command(
        [pixi, "run", "--locked", PACKAGE_NAME],
        cwd=consumer,
        env=CPU_ENV,
        stdout_contains="path=cpu result=332833500",
    )
    installed = [
        json.loads(path.read_text())
        for path in consumer.joinpath(".pixi", "envs", "default", "conda-meta").glob("*.json")
    ]
    installed_by_name = {record["name"]: record for record in installed}
    assert installed_by_name[PACKAGE_NAME]["build"] == cpu_index["build"]
    assert not any("hip" in name for name in installed_by_name)
    for provider, version in [("mojo-compiler", "1.2.0"), ("max-core", "26.7.0")]:
        assert installed_by_name[provider]["version"] == version
        assert installed_by_name[provider]["build"] == _runtime_pin(cpu_index, provider, version)

    for platform in [platforms[0], platforms[2], platforms[3]]:
        incompatible = mojo_workspace.joinpath(f"incompatible-{platform['name']}")
        _write_consumer(incompatible, channel, [platform])
        manifest_path = incompatible.joinpath("pixi.toml")
        manifest = tomli.loads(manifest_path.read_text())
        manifest["dependencies"][PACKAGE_NAME] = {"version": "==1.0.0", "build": amd_index["build"]}
        manifest_path.write_text(tomli_w.dumps(manifest))
        verify_cli_command(
            [pixi, "lock"],
            ExitCode.FAILURE,
            cwd=incompatible,
            stderr_contains="__amdgpu" if platform["name"] == "cpu" else "__amdgpu_arch",
        )

    older_cpu = mojo_workspace.joinpath("older-cpu")
    _write_consumer(
        older_cpu,
        channel,
        [{"name": "v2", "platform": "linux-64", "glibc": "2.34", "archspec": "x86_64_v2"}],
    )
    verify_cli_command(
        [pixi, "lock"],
        ExitCode.FAILURE,
        cwd=older_cpu,
        stderr_contains="_x86_64-microarch-level",
    )
