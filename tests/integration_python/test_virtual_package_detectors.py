from __future__ import annotations

import hashlib
import io
import json
import os
import shlex
import shutil
import tarfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import pytest

from .common import CURRENT_PLATFORM, ExitCode, Output, default_env_path, verify_cli_command


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
                f'{capability.removeprefix("__")} = "{required}" }}'
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
            workspace, "exec", "--channel", channel.url, "--spec", "at-floor", "at-floor"
        ).stdout
    )
    sandbox.run(
        workspace,
        "exec",
        "--channel",
        channel.url,
        "--spec",
        "above-floor",
        "above-floor",
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
        "at-floor",
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


def test_repository_approval_cannot_be_copied_and_local_deny_revokes_it(
    detector_sandbox: DetectorSandbox,
) -> None:
    sandbox = detector_sandbox
    capability = "__repository_capability"
    channel = DetectorChannel(sandbox.root / "channel")
    marker = channel.detector("repository-detect", capability)
    channel.consumer("at-floor", capability, ">=3,<4")
    workspace = sandbox.workspace("repository", [channel], capability, consumer="at-floor")
    sandbox.decision(workspace, channel, "deny")
    local = workspace / ".pixi" / "config.toml"
    local.write_text(
        f'[virtual-package-detectors.consent]\n{json.dumps(channel.url)} = "allow"\n',
        encoding="utf-8",
    )
    assert capability not in versions(sandbox.info(workspace))
    assert not marker.exists()

    sandbox.decision(workspace, channel, "allow", scope="local")
    sandbox.run(workspace, "install")
    assert_report(sandbox.info(workspace), channel, "repository-detect", capability)
    assert marker.exists()

    clone = sandbox.workspace("clone", [channel], capability, consumer="at-floor")
    shutil.copyfile(local, clone / ".pixi" / "config.toml")
    before = marker.read_text()
    sandbox.env["PIXI_CACHE_DIR"] = str(sandbox.root / "clone-cache")
    assert capability not in versions(sandbox.info(clone))
    sandbox.run(clone, "install", expected=ExitCode.FAILURE)
    assert not (default_env_path(clone) / "conda-meta" / "at-floor-1.0-0.json").exists()
    assert marker.read_text() == before

    sandbox.decision(workspace, channel, "deny", scope="local")
    sandbox.decision(workspace, channel, "allow")
    assert capability not in versions(sandbox.info(workspace))
    assert marker.read_text() == before

    # Editing the checkout back to allow cannot restore the revoked approval.
    local.write_text(
        f'[virtual-package-detectors.consent]\n{json.dumps(channel.url)} = "allow"\n',
        encoding="utf-8",
    )
    sandbox.decision(workspace, channel, "deny")
    assert capability not in versions(sandbox.info(workspace))
    assert marker.read_text() == before
