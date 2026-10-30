from __future__ import annotations

import functools
import hashlib
import http.server
import io
import json
import os
import shlex
import shutil
import tarfile
import threading
import tomllib
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import pytest

from .common import (
    CURRENT_PLATFORM,
    ExitCode,
    Output,
    bat_extension,
    default_env_path,
    verify_cli_command,
)

CAPABILITY = "__scope_capability"
KEY = CAPABILITY.removeprefix("__")


@dataclass
class DetectorChannel:
    root: Path
    registrations: dict[str, list[str]] = field(default_factory=dict)
    packages: dict[str, dict[str, Any]] = field(default_factory=dict)

    @property
    def url(self) -> str:
        return self.root.resolve().as_uri() + "/"

    def package(self, name: str, depends: list[str], files: dict[str, bytes] | None = None) -> None:
        files = files or {}
        index = {
            "name": name,
            "version": "1.0",
            "build": "0",
            "build_number": 0,
            "subdir": "noarch",
            "noarch": "generic",
            "depends": depends,
        }
        paths = {
            "paths_version": 1,
            "paths": [
                {
                    "_path": path,
                    "path_type": "hardlink",
                    "sha256": hashlib.sha256(content).hexdigest(),
                    "size_in_bytes": len(content),
                }
                for path, content in files.items()
            ],
        }
        members = {
            "info/index.json": json.dumps(index).encode(),
            "info/paths.json": json.dumps(paths).encode(),
            "info/files": "\n".join(files).encode(),
            **files,
        }
        noarch = self.root / "noarch"
        noarch.mkdir(parents=True, exist_ok=True)
        filename = f"{name}-1.0-0.tar.bz2"
        archive = noarch / filename
        with tarfile.open(archive, "w:bz2") as package:
            for path, content in members.items():
                member = tarfile.TarInfo(path)
                member.size = len(content)
                member.mode = 0o644 if path.startswith("info/") else 0o755
                package.addfile(member, io.BytesIO(content))
        content = archive.read_bytes()
        self.packages[filename] = index | {
            "sha256": hashlib.sha256(content).hexdigest(),
            "md5": hashlib.md5(content).hexdigest(),
            "size": len(content),
        }
        self.write_repodata()

    def detector(self, name: str, capability: str, version: str = "3.0") -> Path:
        marker = self.root / f"{name}.runs"
        report = json.dumps({"version": 1, "virtual_packages": {capability: {"version": version}}})
        self.registrations[name] = [capability]
        self.package(
            name,
            [],
            {
                f"bin/{name}": (
                    f"#!/bin/sh\necho run >> {shlex.quote(str(marker))}\n"
                    f"printf '%s\\n' '{report}'\n"
                ).encode(),
                f"Scripts/{name}.bat": (
                    f'@echo off\r\necho run>>"{marker}"\r\necho {report}\r\n'
                ).encode(),
            },
        )
        return marker

    def consumer(self, name: str, capability: str, constraint: str) -> None:
        self.package(
            name,
            [f"{capability} {constraint}"],
            {
                f"bin/{name}": b"#!/bin/sh\necho consumer-ready\n",
                f"Scripts/{name}.bat": b"@echo off\r\necho consumer-ready\r\n",
            },
        )

    def write_repodata(self) -> None:
        for subdir in (CURRENT_PLATFORM, "noarch"):
            directory = self.root / subdir
            directory.mkdir(parents=True, exist_ok=True)
            (directory / "repodata.json").write_text(
                json.dumps(
                    {
                        "info": {
                            "subdir": subdir,
                            "virtual_package_detectors": self.registrations,
                        },
                        "packages": self.packages if subdir == "noarch" else {},
                        "packages.conda": {},
                        "repodata_version": 1,
                    }
                ),
                encoding="utf-8",
            )


@dataclass
class DetectorSandbox:
    pixi: Path
    root: Path
    env: dict[str, str]

    def run(
        self,
        workspace: Path,
        *args: str,
        expected: ExitCode = ExitCode.SUCCESS,
        overrides: dict[str, str] | None = None,
    ) -> Output:
        return verify_cli_command(
            [self.pixi, *args],
            expected_exit_code=expected,
            cwd=workspace,
            env=self.env | (overrides or {}),
            reset_env=True,
        )

    def decision(
        self, workspace: Path, channel: DetectorChannel, decision: str, scope: str = "shared"
    ) -> None:
        key = f'virtual-package-detectors.consent."{channel.url}"'
        self.run(workspace, "config", "set", f"--{scope}", key, decision)

    def workspace(
        self,
        name: str,
        channels: list[DetectorChannel],
        capability: str | None = None,
        required: str = "3.0",
        consumer: str | None = None,
    ) -> Path:
        workspace = self.root / name
        workspace.mkdir()
        (workspace / ".pixi").mkdir()
        (workspace / ".pixi" / "config.toml").write_text("", encoding="utf-8")
        platform = json.dumps(CURRENT_PLATFORM)
        if capability is not None:
            platform = (
                f'{{ name = "capable", platform = "{CURRENT_PLATFORM}", '
                f'{"cuda" if capability == "__cuda" else capability} = "{required}" }}'
            )
        manifest = (
            '[workspace]\nname = "detector-test"\n'
            f"channels = {json.dumps([channel.url for channel in channels])}\n"
            f"platforms = [{platform}]\n"
            '\n[tasks]\ncheck = "echo selected-fallback-platform"\n'
        )
        if capability is not None:
            manifest += '\n[target.capable.tasks]\ncheck = "echo selected-capable-platform"\n'
        if consumer is not None:
            manifest += f'\n[dependencies]\n{consumer} = "*"\n'
        (workspace / "pixi.toml").write_text(manifest, encoding="utf-8")
        return workspace

    def info(self, workspace: Path, **kwargs: Any) -> dict[str, Any]:
        return json.loads(self.run(workspace, "info", "--json", **kwargs).stdout)


@pytest.fixture
def detector_sandbox(pixi: Path, tmp_pixi_workspace: Path) -> DetectorSandbox:
    root = tmp_pixi_workspace.resolve()
    home = root / "user"
    config_home = home / "config"
    rattler_home = home / "rattler"
    for directory in (config_home, rattler_home):
        directory.mkdir(parents=True)
    config = rattler_home / "config.toml"
    config.write_text("", encoding="utf-8")
    env = {
        "PATH": os.environ.get("PATH", ""),
        "HOME": str(home),
        "USERPROFILE": str(home),
        "APPDATA": str(config_home),
        "LOCALAPPDATA": str(home / "local"),
        "XDG_CONFIG_HOME": str(config_home),
        "XDG_CACHE_HOME": str(home / "cache"),
        "XDG_DATA_HOME": str(home / "data"),
        "RATTLER_HOME": str(rattler_home),
        "PIXI_HOME": str(home / "pixi"),
        "PIXI_CONFIG_FILE": str(config),
        "PIXI_CACHE_DIR": str(root / "cache"),
        "NO_COLOR": "1",
    }
    return DetectorSandbox(pixi.resolve(), root, env)


def versions(info: dict[str, Any]) -> dict[str, str]:
    return {
        parts[0]: parts[1]
        for package in info["virtual_packages"]
        if len(parts := package.split("=")) >= 2
    }


def assert_report(
    info: dict[str, Any], channel: DetectorChannel, detector: str, capability: str
) -> None:
    report = next(
        item for item in info["virtual_package_detectors"] if item["detector"] == detector
    )
    assert report["origin"] == channel.url
    assert report["state"] in {"ran", "cached"}
    assert report["virtual_packages"] == [f"{capability}=3.0=0"]
    assert versions(info)[capability] == "3.0"


@pytest.mark.parametrize("capability", ["__cuda", "__test_detector_capability"])
def test_detector_values_reach_consumers_and_platform_selection(
    detector_sandbox: DetectorSandbox, capability: str
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("capability-detect", capability)
    channel.consumer("at-floor", capability, ">=3,<4 0")
    channel.consumer("above-floor", capability, ">=4,<5")
    workspace = sandbox.workspace("matching", [channel], capability, consumer="at-floor")
    sandbox.decision(workspace, channel, "allow")

    sandbox.run(workspace, "install")
    record_path = default_env_path(workspace) / "conda-meta" / "at-floor-1.0-0.json"
    record = json.loads(record_path.read_text())
    assert record["name"] == "at-floor"
    assert record["version"] == "1.0"
    assert marker.read_text().splitlines() == ["run"]
    assert_report(sandbox.info(workspace), channel, "capability-detect", capability)
    assert "selected-capable-platform" in sandbox.run(workspace, "run", "check").stdout

    # Exec solves against detected values, not the manifest's virtual-package floor.
    assert (
        "consumer-ready"
        in sandbox.run(
            workspace,
            "exec",
            "--channel",
            channel.url,
            "--spec",
            "at-floor",
            bat_extension("at-floor"),
        ).stdout
    )
    sandbox.run(
        workspace,
        "exec",
        "--channel",
        channel.url,
        "--spec",
        "above-floor",
        bat_extension("above-floor"),
        expected=ExitCode.FAILURE,
    )

    unsupported = sandbox.workspace(
        "unsupported", [channel], capability, required="4.0", consumer="above-floor"
    )
    sandbox.run(unsupported, "run", "check", expected=ExitCode.FAILURE)
    assert not default_env_path(unsupported).exists()


@pytest.mark.parametrize("capability", ["__cuda", "__test_detector_capability"])
def test_explicit_override_precedes_detector_execution(
    detector_sandbox: DetectorSandbox, capability: str
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("capability-detect", capability)
    channel.consumer("at-floor", capability, ">=3,<4")
    workspace = sandbox.workspace("override", [channel], capability)
    sandbox.decision(workspace, channel, "allow")
    override = {f"CONDA_OVERRIDE_{capability.removeprefix('__').upper()}": "2.0"}
    assert versions(sandbox.info(workspace, overrides=override))[capability] == "2.0"
    sandbox.run(
        workspace,
        "exec",
        "--channel",
        channel.url,
        "--spec",
        "at-floor",
        bat_extension("at-floor"),
        expected=ExitCode.FAILURE,
        overrides=override,
    )
    assert not marker.exists()


def test_channel_consent_covers_future_detectors_but_not_other_channels(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    trusted = DetectorChannel(sandbox.root / "trusted")
    other = DetectorChannel(sandbox.root / "other")
    first_marker = trusted.detector("first-detect", "__first_capability")
    other_marker = other.detector("other-detect", "__other_capability")
    workspace = sandbox.workspace("first-repository", [trusted, other])
    sandbox.decision(workspace, trusted, "allow")
    assert_report(sandbox.info(workspace), trusted, "first-detect", "__first_capability")
    assert first_marker.exists()
    assert not other_marker.exists()

    second_marker = trusted.detector("second-detect", "__second_capability")
    second_workspace = sandbox.workspace("second-repository", [trusted, other])
    sandbox.env["PIXI_CACHE_DIR"] = str(sandbox.root / "fresh-cache")
    info = sandbox.info(second_workspace)
    assert_report(info, trusted, "first-detect", "__first_capability")
    assert_report(info, trusted, "second-detect", "__second_capability")
    assert second_marker.read_text().splitlines() == ["run"]
    assert not other_marker.exists()
    assert "__other_capability" not in versions(info)

    before = first_marker.read_text(), second_marker.read_text()
    sandbox.decision(second_workspace, trusted, "deny")
    sandbox.decision(second_workspace, other, "allow")
    sandbox.env["PIXI_CACHE_DIR"] = str(sandbox.root / "denied-cache")
    info = sandbox.info(second_workspace)
    assert_report(info, other, "other-detect", "__other_capability")
    assert "__first_capability" not in versions(info)
    assert "__second_capability" not in versions(info)
    assert (first_marker.read_text(), second_marker.read_text()) == before


def test_repository_consent_is_shared_and_never_overrides_a_user_deny(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    capability = "__repository_capability"
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("repository-detect", capability)
    channel.consumer("at-floor", capability, ">=3,<4")
    workspace = sandbox.workspace("repository", [channel], capability, consumer="at-floor")
    key = f'virtual-package-detectors.consent."{channel.url}"'
    sandbox.decision(workspace, channel, "deny")
    local = workspace / ".pixi" / "config.toml"
    sandbox.decision(workspace, channel, "allow", scope="local")
    # The user's deny wins over the repository's allow.
    sandbox.run(workspace, "install", expected=ExitCode.FAILURE)
    assert not marker.exists()
    sandbox.run(workspace, "config", "unset", "--shared", key)
    sandbox.run(workspace, "install")
    assert_report(sandbox.info(workspace), channel, "repository-detect", capability)
    assert marker.exists()

    clone = sandbox.workspace("clone", [channel], capability, consumer="at-floor")
    shutil.copyfile(local, clone / ".pixi" / "config.toml")
    colleague_home = sandbox.root / "colleague"
    colleague_config = colleague_home / "rattler" / "config.toml"
    colleague_config.parent.mkdir(parents=True)
    colleague_config.write_text("", encoding="utf-8")
    colleague = DetectorSandbox(
        sandbox.pixi,
        sandbox.root,
        sandbox.env
        | {
            "HOME": str(colleague_home),
            "USERPROFILE": str(colleague_home),
            "APPDATA": str(colleague_home / "config"),
            "LOCALAPPDATA": str(colleague_home / "local"),
            "XDG_CONFIG_HOME": str(colleague_home / "config"),
            "XDG_CACHE_HOME": str(colleague_home / "cache"),
            "XDG_DATA_HOME": str(colleague_home / "data"),
            "RATTLER_HOME": str(colleague_config.parent),
            "PIXI_HOME": str(colleague_home / "pixi"),
            "PIXI_CONFIG_FILE": str(colleague_config),
            "PIXI_CACHE_DIR": str(sandbox.root / "clone-cache"),
        },
    )
    # Without a decision of their own, a colleague uses the shared allow.
    colleague.run(clone, "install")
    assert_report(colleague.info(clone), channel, "repository-detect", capability)
    assert (default_env_path(clone) / "conda-meta" / "at-floor-1.0-0.json").exists()
    assert marker.read_text().splitlines() == ["run", "run"]

    colleague.decision(clone, channel, "deny", scope="local")
    colleague.decision(clone, channel, "allow")
    before = marker.read_text()
    assert capability not in versions(colleague.info(clone))
    colleague.run(clone, "install", expected=ExitCode.FAILURE)
    assert marker.read_text() == before
    assert_report(sandbox.info(workspace), channel, "repository-detect", capability)

    colleague.run(clone, "config", "unset", "--local", key)
    colleague.run(clone, "install")
    assert_report(colleague.info(clone), channel, "repository-detect", capability)


def test_locked_install_checks_custom_package_upper_bound_on_fresh_and_existing_prefixes(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("scope-detect", CAPABILITY)
    channel.consumer("bounded-consumer", CAPABILITY, ">=1,<2 0")
    workspace = sandbox.workspace("bounded-lock", [channel], CAPABILITY, "1.0", "bounded-consumer")
    sandbox.decision(workspace, channel, "allow")
    sandbox.run(workspace, "lock")
    lock = (workspace / "pixi.lock").read_bytes()

    output = sandbox.run(workspace, "install", "--locked", expected=ExitCode.FAILURE)
    assert CAPABILITY in output.stderr
    record_path = default_env_path(workspace) / "conda-meta" / "bounded-consumer-1.0-0.json"
    assert not record_path.exists()
    assert (workspace / "pixi.lock").read_bytes() == lock
    compatible = {f"CONDA_OVERRIDE_{KEY.upper()}": "1.0"}
    sandbox.run(workspace, "install", "--locked", overrides=compatible)
    assert (
        "consumer-ready"
        in sandbox.run(
            workspace, "run", "--locked", bat_extension("bounded-consumer"), overrides=compatible
        ).stdout
    )
    record = record_path.read_bytes()
    # The current prefix runs without detecting the host again.
    assert (
        "consumer-ready"
        in sandbox.run(workspace, "run", "--locked", bat_extension("bounded-consumer")).stdout
    )
    # Installing detects the host and rejects the existing prefix.
    output = sandbox.run(workspace, "install", "--locked", expected=ExitCode.FAILURE)
    assert CAPABILITY in output.stderr
    assert record_path.read_bytes() == record
    assert (workspace / "pixi.lock").read_bytes() == lock
    assert marker.read_text(encoding="utf-8").splitlines() == ["run"]


def test_locked_run_keeps_the_installed_alias_until_it_is_reinstalled(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    channel.detector("scope-detect", CAPABILITY)
    channel.consumer("high-consumer", CAPABILITY, ">=3,<4 0")
    channel.consumer("bounded-consumer", CAPABILITY, ">=1,<2 0")
    workspace = sandbox.workspace("installed-alias-bounds", [channel])
    (workspace / "pixi.toml").write_text(
        '[workspace]\nname = "installed-alias-bounds"\n'
        f"channels = {json.dumps([channel.url])}\n"
        "platforms = [\n"
        f'  {{ name = "high", platform = "{CURRENT_PLATFORM}", {CAPABILITY} = "3.0" }},\n'
        f'  {{ name = "low", platform = "{CURRENT_PLATFORM}", {CAPABILITY} = "1.0" }},\n'
        "]\n"
        '\n[target.high.dependencies]\nhigh-consumer = "*"\n'
        '\n[target.low.dependencies]\nbounded-consumer = "*"\n',
        encoding="utf-8",
    )
    sandbox.decision(workspace, channel, "allow")
    sandbox.run(workspace, "lock")
    lock = (workspace / "pixi.lock").read_bytes()
    compatible = {f"CONDA_OVERRIDE_{KEY.upper()}": "1.0"}
    sandbox.run(workspace, "install", "--locked", "--platform", "low", overrides=compatible)
    assert (
        "consumer-ready"
        in sandbox.run(
            workspace, "run", "--locked", bat_extension("bounded-consumer"), overrides=compatible
        ).stdout
    )
    # The current prefix runs without detecting the host, also with this
    # machine's own subdir as platform override.
    for overrides in ({}, {"PIXI_OVERRIDE_PLATFORM": CURRENT_PLATFORM}):
        assert (
            "consumer-ready"
            in sandbox.run(
                workspace, "run", "--locked", bat_extension("bounded-consumer"), overrides=overrides
            ).stdout
        )
    # Installing detects the host and picks the alias it satisfies.
    sandbox.run(workspace, "install", "--locked")
    assert (default_env_path(workspace) / "conda-meta" / "high-consumer-1.0-0.json").exists()
    assert (workspace / "pixi.lock").read_bytes() == lock


def test_channel_add_uses_new_effective_detector_scope_before_installing(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    packages = DetectorChannel(sandbox.root / "packages")
    detectors = DetectorChannel(sandbox.root / "detectors")
    packages.consumer("scope-consumer", CAPABILITY, ">=3")
    marker = detectors.detector("scope-detect", CAPABILITY)
    workspace = sandbox.workspace(
        "channel-mutation", [packages], CAPABILITY, consumer="scope-consumer"
    )
    sandbox.decision(workspace, detectors, "allow")
    sandbox.run(workspace, "lock")
    assert not marker.exists()

    sandbox.run(workspace, "workspace", "channel", "add", "--prepend", detectors.url)
    assert marker.exists(), (
        "channel mutation must prepare the new host before the same command installs"
    )
    assert "consumer-ready" in sandbox.run(workspace, "run", bat_extension("scope-consumer")).stdout


@pytest.mark.parametrize(
    "explicit_platform", [False, True], ids=["implicit-host", "declared-platform"]
)
def test_conda_script_target_dependencies_demand_registered_capabilities(
    detector_sandbox: DetectorSandbox, explicit_platform: bool
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("scope-detect", CAPABILITY)
    channel.consumer("script-consumer", CAPABILITY, ">=3,<4")
    workspace = sandbox.workspace("script-scope", [channel])
    sandbox.decision(workspace, channel, "allow")
    script = workspace / "target-dependencies.code"
    metadata = f'channels = ["{channel.url}"]\nentrypoint = "{bat_extension("script-consumer")}"\n'
    if explicit_platform:
        metadata += (
            "\n[tool.pixi.workspace]\n"
            f'platforms = [{{ name = "capable", platform = "{CURRENT_PLATFORM}", {CAPABILITY} = "3.0" }}]\n'
        )
    target = "capable" if explicit_platform else CURRENT_PLATFORM
    metadata += f'\n[tool.pixi.target.{target}.dependencies]\nscript-consumer = "*"\n'
    script.write_text(
        "# /// conda-script\n"
        + "".join(f"# {line}\n" for line in metadata.splitlines())
        + "# /// end-conda-script\n",
        encoding="utf-8",
    )
    sandbox.run(workspace, "lock", "--script", str(script))
    assert (
        "consumer-ready"
        in sandbox.run(
            workspace, "run", "--experimental", "--script", str(script), "--locked"
        ).stdout
    )
    assert marker.read_text(encoding="utf-8").splitlines() == ["run"]


def installed_detector_workspace(sandbox: DetectorSandbox) -> tuple[DetectorChannel, Path, Path]:
    """An installed workspace with an empty detector cache, so any detection runs the detector."""
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("lazy-detect", CAPABILITY)
    channel.consumer("at-floor", CAPABILITY, ">=3")
    workspace = sandbox.workspace("lazy", [channel], CAPABILITY, consumer="at-floor")
    sandbox.decision(workspace, channel, "allow")
    sandbox.run(workspace, "install")
    sandbox.run(workspace, "clean", "cache", "--virtual-package-detectors", "--yes")
    return channel, workspace, marker


def test_commands_on_an_installed_environment_do_not_run_detectors(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    _channel, workspace, marker = installed_detector_workspace(sandbox)
    installed_runs = marker.read_text(encoding="utf-8").splitlines()
    for command in (
        ["task", "list"],
        ["workspace", "name", "get"],
        ["list"],
        ["run", bat_extension("at-floor")],
        ["shell-hook"],
    ):
        sandbox.run(workspace, *command)
        assert marker.read_text(encoding="utf-8").splitlines() == installed_runs, command


def test_installed_environment_runs_where_its_detector_cannot_be_asked(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    channel, workspace, marker = installed_detector_workspace(sandbox)
    installed_runs = marker.read_text(encoding="utf-8").splitlines()
    key = f'virtual-package-detectors.consent."{channel.url}"'
    sandbox.run(workspace, "config", "unset", "--shared", key)

    # Without a terminal or a stored decision, the capability is unknown, which
    # an installed environment does not need.
    assert "consumer-ready" in sandbox.run(workspace, "run", bat_extension("at-floor")).stdout
    assert marker.read_text(encoding="utf-8").splitlines() == installed_runs

    # A new install needs to know the capability.
    shutil.rmtree(default_env_path(workspace))
    output = sandbox.run(workspace, "install", expected=ExitCode.FAILURE)
    assert CAPABILITY in output.stderr


def test_installed_environment_ignores_uninstalled_sibling_detectors(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("scope-detector", CAPABILITY)
    channel.consumer("consumer", CAPABILITY, ">=3")
    workspace = sandbox.workspace("siblings", [channel], CAPABILITY, consumer="consumer")
    manifest = workspace / "pixi.toml"
    manifest.write_text(
        manifest.read_text(encoding="utf-8") + "\n[environments]\ndefault = []\nsibling = []\n",
        encoding="utf-8",
    )
    sandbox.decision(workspace, channel, "allow")
    sandbox.run(workspace, "install")
    sandbox.run(workspace, "clean", "cache", "--virtual-package-detectors", "--yes")
    installed_runs = marker.read_text(encoding="utf-8").splitlines()
    for command in (
        ["run", "--locked", "-e", "default", "check"],
        ["list", "--locked", "-e", "default"],
        ["tree", "--locked", "-e", "default"],
        ["shell-hook", "--locked", "-e", "default"],
    ):
        sandbox.run(workspace, *command)
        assert marker.read_text(encoding="utf-8").splitlines() == installed_runs, command


@pytest.fixture
def installed_default_with_http_sibling(
    detector_sandbox: DetectorSandbox,
) -> Iterator[tuple[Path, list[str], Path]]:
    sandbox = detector_sandbox
    base = DetectorChannel(sandbox.root / "base-channel")
    base.package("base-package", [])
    sibling = DetectorChannel(sandbox.root / "sibling-channel")
    marker = sibling.detector("sibling-detect", CAPABILITY)
    sibling.consumer("sibling-consumer", CAPABILITY, ">=3")
    requests: list[str] = []

    class Handler(http.server.SimpleHTTPRequestHandler):
        def do_GET(self) -> None:
            requests.append(self.path)
            super().do_GET()

        def do_HEAD(self) -> None:
            requests.append(self.path)
            super().do_HEAD()

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(Handler, directory=str(sibling.root))
    )
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        sibling_url = f"http://127.0.0.1:{server.server_address[1]}/"
        workspace = sandbox.workspace("http-siblings", [base])
        (workspace / "pixi.toml").write_text(
            '[workspace]\nname = "http-siblings"\n'
            f"channels = {json.dumps([base.url])}\n"
            "platforms = [\n"
            f'  "{CURRENT_PLATFORM}",\n'
            f'  {{ name = "capable", platform = "{CURRENT_PLATFORM}", {CAPABILITY} = "3.0" }},\n'
            "]\n"
            '\n[dependencies]\nbase-package = "*"\n'
            '\n[tasks]\ncheck = "echo default-ready"\n'
            'workflow = { cmd = "echo workflow-ready", '
            'depends-on = [{ task = "child", environment = "sibling" }] }\n'
            "\n[feature.sibling]\n"
            f"channels = {json.dumps([sibling_url])}\n"
            'platforms = ["capable"]\n'
            '\n[feature.sibling.dependencies]\nsibling-consumer = "*"\n'
            "\n[feature.sibling.target.capable.tasks]\n"
            'child = "echo detected-child-ready"\n'
            "\n[environments]\n"
            f'default = {{ features = [], platforms = ["{CURRENT_PLATFORM}"] }}\n'
            'sibling = ["sibling"]\n',
            encoding="utf-8",
        )
        sandbox.run(
            workspace,
            "config",
            "set",
            "--shared",
            f'virtual-package-detectors.consent."{sibling_url}"',
            "allow",
        )
        sandbox.run(workspace, "lock")
        assert requests, "the counting server must observe channel discovery during locking"
        sandbox.run(workspace, "install", "--locked", "-e", "default")
        assert (default_env_path(workspace) / "conda-meta" / "base-package-1.0-0.json").exists()
        assert not (workspace / ".pixi" / "envs" / "sibling").exists()
        marker.unlink(missing_ok=True)
        requests.clear()
        # A fresh cache makes every repodata request visible.
        sandbox.env["PIXI_CACHE_DIR"] = str(sandbox.root / "run-cache")
        yield workspace, requests, marker
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def test_frozen_implicit_executable_does_not_discover_uninstalled_sibling_channel(
    detector_sandbox: DetectorSandbox,
    installed_default_with_http_sibling: tuple[Path, list[str], Path],
) -> None:
    workspace, requests, marker = installed_default_with_http_sibling
    lock = (workspace / "pixi.lock").read_bytes()
    output = detector_sandbox.run(workspace, "run", "--frozen", "echo", "executable-ready")
    assert "executable-ready" in output.stdout
    assert requests == []
    assert not marker.exists()
    assert (workspace / "pixi.lock").read_bytes() == lock
    assert (default_env_path(workspace) / "conda-meta" / "base-package-1.0-0.json").exists()
    assert not (workspace / ".pixi" / "envs" / "sibling").exists()


def test_frozen_inherited_task_does_not_discover_uninstalled_sibling_channel(
    detector_sandbox: DetectorSandbox,
    installed_default_with_http_sibling: tuple[Path, list[str], Path],
) -> None:
    workspace, requests, marker = installed_default_with_http_sibling
    lock = (workspace / "pixi.lock").read_bytes()
    output = detector_sandbox.run(workspace, "run", "--frozen", "check")
    assert "default-ready" in output.stdout
    assert requests == []
    assert not marker.exists()
    assert (workspace / "pixi.lock").read_bytes() == lock
    assert (default_env_path(workspace) / "conda-meta" / "base-package-1.0-0.json").exists()
    assert not (workspace / ".pixi" / "envs" / "sibling").exists()


def test_frozen_implicit_task_selects_detector_dependent_sibling(
    detector_sandbox: DetectorSandbox,
    installed_default_with_http_sibling: tuple[Path, list[str], Path],
) -> None:
    workspace, requests, marker = installed_default_with_http_sibling
    lock = (workspace / "pixi.lock").read_bytes()
    output = detector_sandbox.run(workspace, "run", "--frozen", "child")
    assert "detected-child-ready" in output.stdout
    assert requests
    assert marker.read_text(encoding="utf-8").splitlines() == ["run"]
    assert (
        workspace / ".pixi" / "envs" / "sibling" / "conda-meta" / "sibling-consumer-1.0-0.json"
    ).exists()
    assert (default_env_path(workspace) / "conda-meta" / "base-package-1.0-0.json").exists()
    assert (workspace / "pixi.lock").read_bytes() == lock


def test_frozen_cross_environment_task_detects_only_when_dependency_is_reached(
    detector_sandbox: DetectorSandbox,
    installed_default_with_http_sibling: tuple[Path, list[str], Path],
) -> None:
    sandbox = detector_sandbox
    workspace, requests, marker = installed_default_with_http_sibling
    lock = (workspace / "pixi.lock").read_bytes()
    output = sandbox.run(workspace, "run", "--frozen", "--skip-deps", "workflow")
    assert "workflow-ready" in output.stdout
    assert "detected-child-ready" not in output.stdout
    assert requests == []
    assert not marker.exists()
    assert not (workspace / ".pixi" / "envs" / "sibling").exists()
    assert (workspace / "pixi.lock").read_bytes() == lock

    output = sandbox.run(workspace, "run", "--frozen", "workflow")
    assert "detected-child-ready" in output.stdout
    assert "workflow-ready" in output.stdout
    assert requests
    assert marker.read_text(encoding="utf-8").splitlines() == ["run"]
    assert (
        workspace / ".pixi" / "envs" / "sibling" / "conda-meta" / "sibling-consumer-1.0-0.json"
    ).exists()
    assert (default_env_path(workspace) / "conda-meta" / "base-package-1.0-0.json").exists()
    assert (workspace / "pixi.lock").read_bytes() == lock


def info_workspace(sandbox: DetectorSandbox, channel: DetectorChannel, scope: str) -> Path:
    workspace = sandbox.workspace("info-scope", [channel] if scope == "default" else [])
    (workspace / "pixi.toml").write_text(
        '[workspace]\nname = "info-scope"\n'
        f"channels = {json.dumps([channel.url] if scope == 'default' else [])}\n"
        "platforms = [\n"
        f'  {{ name = "capable", platform = "{CURRENT_PLATFORM}", {CAPABILITY} = "3.0" }},\n'
        f'  {{ name = "fallback", platform = "{CURRENT_PLATFORM}" }},\n'
        "]\n"
        '\n[target.capable.tasks]\ncapable-task = "echo capable"\n'
        '\n[target.fallback.tasks]\nfallback-task = "echo fallback"\n'
        '\n[target.capable.dependencies]\ncapable-dependency = "*"\n'
        '\n[target.fallback.dependencies]\nfallback-dependency = "*"\n'
        '\n[target.capable.pypi-dependencies]\ncapable-python = "*"\n'
        '\n[target.fallback.pypi-dependencies]\nfallback-python = "*"\n'
        "\n[feature.site]\n"
        f"channels = {json.dumps([channel.url])}\n"
        '\n[environments]\nsite = ["site"]\nsite-copy = ["site"]\nmirror = []\n',
        encoding="utf-8",
    )
    return workspace


def environment_info(info: dict[str, Any], name: str) -> dict[str, Any]:
    return next(
        environment for environment in info["environments_info"] if environment["name"] == name
    )


def assert_selected(environment: dict[str, Any], platform: str) -> None:
    assert environment["tasks"] == [f"{platform}-task"]
    assert environment["dependencies"] == [f"{platform}-dependency"]
    assert environment["pypi_dependencies"] == [f"{platform}-python"]


@pytest.mark.parametrize("scope", ["default", "site"])
def test_info_uses_effective_environment_detection_once(
    detector_sandbox: DetectorSandbox, scope: str
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("info-detect", CAPABILITY)
    workspace = info_workspace(sandbox, channel, scope)
    sandbox.decision(workspace, channel, "allow")

    info = sandbox.info(workspace)
    assert_selected(environment_info(info, "site"), "capable")
    assert_selected(environment_info(info, "site-copy"), "capable")
    expected_default = "capable" if scope == "default" else "fallback"
    assert_selected(environment_info(info, "default"), expected_default)
    assert_selected(environment_info(info, "mirror"), expected_default)
    assert marker.read_text(encoding="utf-8").splitlines() == ["run"]
    if scope == "default":
        assert versions(info)[CAPABILITY] == "3.0"
        report = next(
            item for item in info["virtual_package_detectors"] if item["detector"] == "info-detect"
        )
        assert report["state"] == "ran"
    else:
        assert CAPABILITY not in versions(info)
    assert "virtual_package_detection_error" not in info


@pytest.mark.parametrize("scope", ["default", "site"])
def test_info_detection_errors_remain_inline(detector_sandbox: DetectorSandbox, scope: str) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    channel.detector("info-detect", CAPABILITY)
    workspace = info_workspace(sandbox, channel, scope)
    sandbox.decision(workspace, channel, "allow")
    overrides = {f"CONDA_OVERRIDE_{KEY.upper()}": "not a valid version !"}

    info = sandbox.info(workspace, overrides=overrides)
    assert "virtual_package_detection_error" in environment_info(info, "site")
    assert_selected(environment_info(info, "site"), "fallback")
    if scope == "default":
        assert "virtual_package_detection_error" in info
    else:
        assert "virtual_package_detection_error" not in info
    output = sandbox.run(workspace, "info", overrides=overrides)
    assert "Detection error" in output.stdout


@pytest.mark.parametrize("destination", ["feature", "environment", "solve-group"])
def test_auto_detect_uses_destination_channels(
    detector_sandbox: DetectorSandbox, destination: str
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "site-channel")
    marker = channel.detector("site-detect", "__site_capability")
    workspace = sandbox.workspace("auto-detect-scope", [])
    manifest = (
        '[workspace]\nname = "auto-detect-scope"\nchannels = []\n'
        f'platforms = ["{CURRENT_PLATFORM}"]\n'
        "\n[feature.site]\n"
        f"channels = {json.dumps([channel.url])}\n"
        "\n[environments]\n"
    )
    if destination == "solve-group":
        manifest += (
            'site = { features = [], no-default-feature = true, solve-group = "shared" }\n'
            'supplier = { features = ["site"], no-default-feature = true, solve-group = "shared" }\n'
        )
    else:
        manifest += 'site = { features = ["site"], no-default-feature = true }\n'
    (workspace / "pixi.toml").write_text(manifest, encoding="utf-8")
    sandbox.decision(workspace, channel, "allow")

    sandbox.run(
        workspace,
        "workspace",
        "platform",
        "add",
        "site-host",
        "--auto-detect",
        "--no-install",
        "--feature" if destination == "feature" else "--environment",
        "site",
    )
    manifest_data = tomllib.loads((workspace / "pixi.toml").read_text(encoding="utf-8"))
    platform = next(
        platform
        for platform in manifest_data["workspace"]["platforms"]
        if isinstance(platform, dict) and platform["name"] == "site-host"
    )
    assert platform["__site_capability"] == "3.0"
    assert marker.read_text(encoding="utf-8").splitlines() == ["run"]
    destination_data = (
        manifest_data["feature"]["site"]
        if destination == "feature"
        else manifest_data["environments"]["site"]
    )
    assert "site-host" in destination_data["platforms"]


def test_info_platform_label_honors_target_override(detector_sandbox: DetectorSandbox) -> None:
    sandbox = detector_sandbox
    workspace = sandbox.workspace("override-label", [])
    overrides = {"PIXI_OVERRIDE_PLATFORM": "osx-64", "CONDA_OVERRIDE_OSX": "14.0"}
    info = sandbox.info(workspace, overrides=overrides)
    assert info["platform"] == "osx-64"
    assert versions(info)["__osx"] == "14.0"
    assert "__linux" not in versions(info)
    assert "osx-64" in sandbox.run(workspace, "info", overrides=overrides).stdout


@pytest.mark.parametrize("source", ["builtin-override", "detector"])
def test_cached_builtin_requires_provenance_after_sibling_probes(
    detector_sandbox: DetectorSandbox, source: str
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("cuda-detect", "__cuda", "999999")
    channel.consumer("consumer", "__cuda", ">=999999")
    workspace = sandbox.workspace("cached-consulted", [channel], "__cuda", "999999", "consumer")
    with (workspace / "pixi.toml").open("a", encoding="utf-8") as manifest:
        manifest.write("\n[environments]\nsibling = []\n")
    if source == "detector":
        sandbox.decision(workspace, channel, "allow")
        sandbox.run(workspace, "install")
        sandbox.run(
            workspace,
            "config",
            "unset",
            "--shared",
            f'virtual-package-detectors.consent."{channel.url}"',
        )
    else:
        sandbox.run(workspace, "install", overrides={"CONDA_OVERRIDE_CUDA": "999999"})
    installed_runs = marker.read_text(encoding="utf-8") if marker.exists() else ""
    sandbox.run(workspace, "clean", "cache", "--virtual-package-detectors", "--yes")
    output = sandbox.run(
        workspace,
        "run",
        "--locked",
        bat_extension("consumer"),
        expected=ExitCode.SUCCESS if source == "detector" else ExitCode.FAILURE,
    )
    if source == "detector":
        assert "consumer-ready" in output.stdout
    else:
        assert "__cuda" in output.stderr
    assert (marker.read_text(encoding="utf-8") if marker.exists() else "") == installed_runs


@pytest.mark.parametrize("command", ["run", "shell-hook"])
def test_installed_prefix_checks_explicitly_absent_builtin_cuda(
    detector_sandbox: DetectorSandbox, command: str
) -> None:
    sandbox = detector_sandbox
    channel = DetectorChannel(sandbox.root / "channel")
    channel.consumer("cuda-client", "__cuda", ">=3")
    workspace = sandbox.workspace("builtin-absence", [channel], "__cuda", "3.0", "cuda-client")
    sandbox.run(workspace, "install", overrides={"CONDA_OVERRIDE_CUDA": "3.0"})
    lock = (workspace / "pixi.lock").read_bytes()
    record = default_env_path(workspace) / "conda-meta" / "cuda-client-1.0-0.json"
    installed_record = record.read_bytes()

    args = (
        ("run", "--locked", bat_extension("cuda-client"))
        if command == "run"
        else ("shell-hook", "--locked")
    )
    output = sandbox.run(
        workspace,
        *args,
        overrides={"CONDA_OVERRIDE_CUDA": ""},
        expected=ExitCode.FAILURE,
    )
    assert "__cuda" in output.stderr
    assert "consumer-ready" not in output.stdout
    assert (workspace / "pixi.lock").read_bytes() == lock
    assert record.read_bytes() == installed_record
