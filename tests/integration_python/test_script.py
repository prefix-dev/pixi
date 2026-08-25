import json
import shutil
import threading
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Iterator

import pytest
from inline_snapshot import snapshot

from .common import CONDA_FORGE_CHANNEL, CURRENT_PLATFORM, ExitCode, verify_cli_command


def assert_no_workspace_state_created(workspace: Path) -> None:
    assert {path.name for path in (workspace / ".pixi").iterdir()} == {"config.toml"}


@contextmanager
def remote_script_server(source: str) -> Iterator[tuple[str, list[str]]]:
    requests: list[str] = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            requests.append(self.path)
            if self.path == "/redirect":
                self.send_response(302)
                self.send_header(
                    "Location",
                    f"http://127.0.0.1:{server.server_port}/extensionless",
                )
                self.end_headers()
                return
            if self.path == "/extensionless":
                body = source.encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            self.send_response(404)
            self.end_headers()

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}", requests
    finally:
        server.shutdown()
        thread.join()
        server.server_close()


def test_pixi_init_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "scripts" / "example.py"
    script.parent.mkdir()
    script.write_text("#!/usr/bin/env python\nprint('hello')\n")

    verify_cli_command([pixi, "init", "--script", script, "--channel", "testing"])

    assert (
        script.read_text()
        == """#!/usr/bin/env python
#
# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["testing"]
# ///

print('hello')
"""
    )
    assert not (tmp_pixi_workspace / "pixi.toml").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)

    verify_cli_command(
        [pixi, "init", "--script", script],
        ExitCode.FAILURE,
        stderr_contains="already a PEP 723 script",
    )


def test_pixi_run_script_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "run", "--script", script],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi init --script",
        ],
    )
    assert script.read_text() == "print('hello')\n"


@pytest.mark.slow
def test_pixi_run_remote_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    source = f'''# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# ///
import json
import os
import sys

print(json.dumps({{
    "argv": sys.argv[1:],
    "cwd": os.getcwd(),
    "file": __file__,
}}))
'''
    with remote_script_server(source) as (base_url, requests):
        output = verify_cli_command(
            [
                pixi,
                "run",
                "--script",
                f"{base_url}/redirect",
                "first",
                "--second",
            ],
            cwd=tmp_pixi_workspace,
        )

    payload = json.loads(next(line for line in output.stdout.splitlines() if line.startswith("{")))
    assert payload["argv"] == ["first", "--second"]
    assert payload["cwd"] == str(tmp_pixi_workspace)
    assert payload["file"].endswith(".py")
    assert not Path(payload["file"]).exists()
    assert requests == ["/redirect", "/extensionless"]
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_run_remote_script_reports_http_errors_and_rejects_locks(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    with remote_script_server("") as (base_url, requests):
        verify_cli_command(
            [pixi, "run", "--script", f"{base_url}/missing"],
            ExitCode.FAILURE,
            stderr_contains=["server returned 404", f"{base_url}/missing"],
        )
        verify_cli_command(
            [pixi, "run", "--frozen", "--script", f"{base_url}/extensionless"],
            ExitCode.FAILURE,
            stderr_contains=[
                "transient scripts cannot be run with `--frozen`",
                "do not have an adjacent lock file",
            ],
        )
    assert requests == ["/missing"]


@pytest.mark.slow
def test_pixi_run_stdin_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    exec_cache = tmp_pixi_workspace / "stdin-exec-cache"
    env = {"PIXI_CACHE_EXEC_ENVIRONMENTS_DIR": str(exec_cache)}
    source = f'''# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# ///
import json
import os
import sys

print(json.dumps({{
    "argv": sys.argv,
    "cwd": os.getcwd(),
    "has_file": "__file__" in globals(),
    "manifest": os.environ["PIXI_PROJECT_MANIFEST"],
    "remaining_stdin": sys.stdin.read(),
}}))
'''
    output = verify_cli_command(
        [pixi, "run", "--script", "-", "first", "--second"],
        cwd=tmp_pixi_workspace,
        env=env,
        stdin=source,
    )

    payload = json.loads(next(line for line in output.stdout.splitlines() if line.startswith("{")))
    assert payload == {
        "argv": ["-c", "first", "--second"],
        "cwd": str(tmp_pixi_workspace),
        "has_file": False,
        "manifest": "<stdin>",
        "remaining_stdin": "",
    }

    body_changed = source.replace("import json", "# changed body\nimport json")
    verify_cli_command(
        [pixi, "run", "--script", "-", "first", "--second"],
        cwd=tmp_pixi_workspace,
        env=env,
        stdin=body_changed,
    )
    assert len(list(exec_cache.iterdir())) == 1

    metadata_changed = source.replace(
        "# ///\nimport json", "# # identity change\n# ///\nimport json"
    )
    verify_cli_command(
        [pixi, "run", "--script", "-", "first", "--second"],
        cwd=tmp_pixi_workspace,
        env=env,
        stdin=metadata_changed,
    )
    assert len(list(exec_cache.iterdir())) == 2
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_run_stdin_script_errors_and_dry_run_are_source_safe(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    verify_cli_command(
        [pixi, "run", "--script", "-"],
        ExitCode.FAILURE,
        stderr_contains="stdin does not contain a PEP 723 metadata block",
        stdin="print('missing metadata')\n",
    )
    verify_cli_command(
        [pixi, "run", "--script", "-"],
        ExitCode.FAILURE,
        stderr_contains="<stdin>",
        stderr_excludes="stdin.py",
        stdin="# /// script\n# dependencies = [\n# ///\n",
    )

    secret_marker = "stdin-body-must-not-appear"
    source = f'''# /// script
# dependencies = []
# ///
print("{secret_marker}")
    '''
    verify_cli_command(
        [pixi, "-vvv", "run", "--dry-run", "--script", "-"],
        cwd=tmp_pixi_workspace,
        stdin=source,
        stdout_excludes=secret_marker,
        stderr_contains="python -c <stdin>",
        stderr_excludes=secret_marker,
    )
    verify_cli_command(
        [pixi, "run", "--locked", "--script", "-"],
        ExitCode.FAILURE,
        stderr_contains=[
            "transient scripts cannot be run with `--locked`",
            "do not have an adjacent lock file",
        ],
        stdin=secret_marker,
        stderr_excludes=secret_marker,
    )


def test_pixi_run_script_rejects_workspace_only_options(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        """# /// script
# dependencies = []
# ///
print("hello")
"""
    )
    original_script = script.read_text()

    for option in (["--environment", "test"], ["--skip-deps"]):
        verify_cli_command(
            [pixi, "run", "--script", script, *option],
            ExitCode.FAILURE,
            stderr_contains=[
                f"does not support {option[0]}",
                "one implicit default run environment and no Pixi task graph",
            ],
        )

    assert script.read_text() == original_script
    assert not script.with_name("example.py.pixi.lock").exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_lock_script_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "lock", "--script", script],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi init --script",
        ],
    )

    assert script.read_text() == "print('hello')\n"
    assert not script.with_name("example.py.pixi.lock").exists()


@pytest.mark.slow
def test_pixi_run_script_is_isolated_and_does_not_create_a_lock(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    (tmp_pixi_workspace / "pixi.toml").write_text(
        f'''[workspace]
name = "enclosing"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
'''
    )
    script = tmp_pixi_workspace / "scripts" / "example.py"
    script.parent.mkdir()
    script.write_text(
        """# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["conda-forge"]
#
# [tool.pixi.dependencies]
# ///
import json
import os
import sys

print(json.dumps({
    "argv": sys.argv[1:],
    "cwd": os.getcwd(),
    "manifest": os.environ["PIXI_PROJECT_MANIFEST"],
}))
"""
    )

    verify_cli_command(
        [pixi, "run", "--script", script, "first", "--second"],
        cwd=tmp_pixi_workspace,
        env={
            "PIXI_PROJECT_ROOT": str(tmp_pixi_workspace),
            "PIXI_ENVIRONMENT_NAME": "ignored",
        },
        stdout_contains=json.dumps(
            {
                "argv": ["first", "--second"],
                "cwd": str(tmp_pixi_workspace),
                "manifest": str(script),
            }
        ),
    )

    assert not script.with_name("example.py.pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


@pytest.mark.slow
def test_pixi_run_script_reuses_a_satisfying_resolution_without_solving(
    pixi: Path, tmp_pixi_workspace: Path, channels: Path
) -> None:
    """A satisfying cached resolution does not require channel access."""
    exec_cache = tmp_pixi_workspace / "script-exec-cache"
    repodata_cache = tmp_pixi_workspace / "repodata-cache"
    env = {
        "PIXI_CACHE_EXEC_ENVIRONMENTS_DIR": str(exec_cache),
        "PIXI_CACHE_REPODATA_DIR": str(repodata_cache),
    }
    channel = tmp_pixi_workspace / "channel"
    shutil.copytree(channels / "multiple_versions_channel_1", channel)
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{channel.as_uri()}", "{CONDA_FORGE_CHANNEL}"]
# platforms = ["{CURRENT_PLATFORM}"]
#
# [tool.pixi.dependencies]
# package = "0.1.*"
# ///
print("SCRIPT-RAN")
'''
    )

    verify_cli_command(
        [pixi, "run", "--script", script],
        cwd=tmp_pixi_workspace,
        env=env,
        stdout_contains="SCRIPT-RAN",
    )

    package_records = list(exec_cache.glob("*/envs/default/conda-meta/package-*.json"))
    assert len(package_records) == 1
    assert json.loads(package_records[0].read_text())["version"] == "0.1.0"

    script.write_text(script.read_text().replace('package = "0.1.*"', 'package = "*"'))
    shutil.rmtree(channel)
    shutil.rmtree(repodata_cache)
    verify_cli_command(
        [pixi, "run", "--script", script],
        cwd=tmp_pixi_workspace,
        env=env,
        stdout_contains="SCRIPT-RAN",
    )

    package_records = list(exec_cache.glob("*/envs/default/conda-meta/package-*.json"))
    assert len(package_records) == 1
    assert json.loads(package_records[0].read_text())["version"] == "0.1.0"
    assert not script.with_name("example.py.pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


@pytest.mark.slow
def test_pixi_lock_script_writes_only_the_adjacent_lock(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    (tmp_pixi_workspace / "pixi.toml").write_text(
        f'''[workspace]
name = "enclosing"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
'''
    )
    script = tmp_pixi_workspace / "scripts" / "example.py"
    script.parent.mkdir()
    script.write_text(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.pixi.dependencies]
# ///
print("hello")
'''
    )
    original_script = script.read_text()
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command([pixi, "lock", "--script", script, "--dry-run"], cwd=tmp_pixi_workspace)
    assert script.read_text() == original_script
    assert not script_lock.exists()

    verify_cli_command([pixi, "lock", "--script", script], cwd=tmp_pixi_workspace)
    assert script.read_text() == original_script
    assert script_lock.exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_add_script_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "add", "--script", script, "rich"],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi init --script",
        ],
    )

    assert script.read_text() == "print('hello')\n"
    assert not script.with_name("example.py.pixi.lock").exists()


def test_pixi_dependency_mutations_reject_workspace_only_options(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        """# /// script
# dependencies = []
# ///
print("hello")
"""
    )
    original_script = script.read_text()

    for command, options in [
        ("add", ["--feature", "test", "--host"]),
        ("add", ["--environment", "test"]),
        ("remove", ["--feature", "test", "--build"]),
        ("remove", ["--environment", "test"]),
    ]:
        verify_cli_command(
            [pixi, command, "--script", script, *options, "bzip2"],
            ExitCode.FAILURE,
            stderr_contains=[
                f"`pixi {command} --script` does not support",
                options[0],
                "one implicit default run environment",
            ],
        )

    assert script.read_text() == original_script
    assert not script.with_name("example.py.pixi.lock").exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


@pytest.mark.slow
def test_pixi_add_script_writes_conda_and_pypi_dependencies(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command(
        [pixi, "add", "--script", script, "--no-install", "bzip2"],
        cwd=tmp_pixi_workspace,
        stderr_contains="Added bzip2",
    )
    assert not script_lock.exists()

    verify_cli_command([pixi, "lock", "--script", script], cwd=tmp_pixi_workspace)
    original_lock = script_lock.read_text()

    verify_cli_command(
        [
            pixi,
            "add",
            "--script",
            script,
            "--no-install",
            "--pypi",
            "requests==2.32.5",
        ],
        cwd=tmp_pixi_workspace,
        stderr_contains="Added requests==2.32.5",
    )

    assert script.read_text() == snapshot(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = ["requests==2.32.5"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.pixi.dependencies]
# bzip2 = "*"
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    assert script_lock.read_text() != original_lock
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


@pytest.mark.slow
def test_pixi_add_script_writes_representable_dependency_options(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# platforms = ["{CURRENT_PLATFORM}"]
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    editable = tmp_pixi_workspace / "demo-editable"
    editable.mkdir()
    (editable / "pyproject.toml").write_text(
        """[project]
name = "demo-editable"
version = "0.1.0"
"""
    )
    source = tmp_pixi_workspace / "demo-source"
    source.mkdir()
    (source / "pyproject.toml").write_text(
        """[project]
name = "demo-source"
version = "0.1.0"
"""
    )

    verify_cli_command(
        [
            pixi,
            "add",
            "--script",
            script,
            "--no-install",
            "--platform",
            CURRENT_PLATFORM,
            "zlib",
        ],
        cwd=tmp_pixi_workspace,
        stderr_contains=["Added zlib", f"platform(s): {CURRENT_PLATFORM}"],
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--script",
            script,
            "--no-install",
            "--pypi",
            "--index",
            "https://pypi.org/simple",
            "requests==2.32.5",
        ],
        cwd=tmp_pixi_workspace,
        stderr_contains="Added requests==2.32.5",
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--script",
            script,
            "--no-install",
            "--pypi",
            "--editable",
            "demo-editable @ ./demo-editable",
        ],
        cwd=tmp_pixi_workspace,
        stderr_contains="Added demo-editable @ ./demo-editable",
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--script",
            script,
            "--no-install",
            "--pypi",
            "demo-source @ ./demo-source",
        ],
        cwd=tmp_pixi_workspace,
        stderr_contains="Added demo-source @ ./demo-source",
    )

    assert script.read_text() == snapshot(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = ["demo-source @ {source.as_uri()}"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# platforms = ["{CURRENT_PLATFORM}"]
#
# [tool.pixi.target.{CURRENT_PLATFORM}.dependencies]
# zlib = "*"
#
# [tool.pixi.pypi-dependencies]
# requests = {{ version = "==2.32.5", index = "https://pypi.org/simple" }}
# demo-editable = {{ path = "./demo-editable", editable = true }}
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    verify_cli_command(
        [
            pixi,
            "remove",
            "--script",
            script,
            "--no-install",
            "--platform",
            CURRENT_PLATFORM,
            "zlib",
        ],
        cwd=tmp_pixi_workspace,
        stderr_contains="Removed zlib",
    )
    assert script.read_text() == snapshot(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = ["demo-source @ {source.as_uri()}"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# platforms = ["{CURRENT_PLATFORM}"]
#
# [tool.pixi.pypi-dependencies]
# requests = {{ version = "==2.32.5", index = "https://pypi.org/simple" }}
# demo-editable = {{ path = "./demo-editable", editable = true }}
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    assert not script.with_name("example.py.pixi.lock").exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_remove_script_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "remove", "--script", script, "requests"],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi init --script",
        ],
    )
    assert script.read_text() == "print('hello')\n"


def test_pixi_workspace_channel_edits_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        """# /// script
# dependencies = []
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
"""
    )
    original_script = script.read_text()

    verify_cli_command(
        [
            pixi,
            "workspace",
            "channel",
            "add",
            "--script",
            script,
            "--feature",
            "test",
            "--no-install",
            "conda-forge",
        ],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not support --feature",
            "one implicit default run environment",
        ],
    )
    assert script.read_text() == original_script

    verify_cli_command(
        [
            pixi,
            "workspace",
            "channel",
            "add",
            "--script",
            script,
            "--no-install",
            "conda-forge",
        ],
        stderr_contains="Added conda-forge",
    )
    assert script.read_text() == snapshot(
        """# /// script
# dependencies = []
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.pixi.workspace]
# channels = ["conda-forge"]
# ///
print("hello")
"""
    )
    assert not script.with_name("example.py.pixi.lock").exists()

    verify_cli_command(
        [pixi, "workspace", "channel", "list", "--script", script],
        stdout_contains=["Environment: default", "- conda-forge"],
    )

    verify_cli_command(
        [
            pixi,
            "workspace",
            "channel",
            "remove",
            "--script",
            script,
            "--no-install",
            "conda-forge",
        ],
        stderr_contains="Removed conda-forge",
    )
    assert script.read_text() == snapshot(
        """# /// script
# dependencies = []
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.pixi.workspace]
# channels = []
# ///
print("hello")
"""
    )
    assert not script.with_name("example.py.pixi.lock").exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_workspace_platform_edits_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        """# /// script
# dependencies = []
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.pixi.workspace]
# platforms = ["linux-aarch64"]
# ///
print("hello")
"""
    )
    original_script = script.read_text()

    verify_cli_command(
        [
            pixi,
            "workspace",
            "platform",
            "add",
            "--script",
            script,
            "--feature",
            "test",
            "--no-install",
            "linux-ci=linux-64",
        ],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not support --feature",
            "one implicit default run environment",
        ],
    )
    assert script.read_text() == original_script

    verify_cli_command(
        [
            pixi,
            "workspace",
            "platform",
            "add",
            "--script",
            script,
            "--no-install",
            "linux-ci=linux-64",
            "mac-ci=osx-64",
        ],
        stderr_contains=["Added linux-ci", "Added mac-ci"],
    )
    assert script.read_text() == snapshot(
        """# /// script
# dependencies = []
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.pixi.workspace]
# platforms = ["linux-aarch64", { name = "linux-ci", platform = "linux-64" }, { name = "mac-ci", platform = "osx-64" }]
# ///
print("hello")
"""
    )

    verify_cli_command(
        [
            pixi,
            "workspace",
            "platform",
            "edit",
            "--script",
            script,
            "linux-ci",
            "--cuda",
            "12.0",
            "--no-install",
        ],
        stderr_contains="Updated platform linux-ci",
    )
    assert script.read_text() == snapshot(
        """# /// script
# dependencies = []
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.pixi.workspace]
# platforms = ["linux-aarch64", { name = "linux-ci", platform = "linux-64", cuda = "12.0" }, { name = "mac-ci", platform = "osx-64" }]
# ///
print("hello")
"""
    )

    verify_cli_command(
        [
            pixi,
            "workspace",
            "platform",
            "move",
            "--script",
            script,
            "mac-ci",
            "--to-top",
            "--no-install",
        ],
        stderr_contains="Moved platform mac-ci",
    )
    assert script.read_text() == snapshot(
        """# /// script
# dependencies = []
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.pixi.workspace]
# platforms = [{ name = "mac-ci", platform = "osx-64" }, "linux-aarch64", { name = "linux-ci", platform = "linux-64", cuda = "12.0" }]
# ///
print("hello")
"""
    )

    verify_cli_command(
        [
            pixi,
            "workspace",
            "platform",
            "list",
            "--script",
            script,
            "--machine-readable",
        ],
        stdout_contains="mac-ci linux-aarch64 linux-ci",
    )

    verify_cli_command(
        [
            pixi,
            "workspace",
            "platform",
            "remove",
            "--script",
            script,
            "--no-install",
            "mac-ci",
            "linux-aarch64",
            "linux-ci",
        ],
        stderr_contains=["Removed mac-ci", "Removed linux-ci"],
    )
    assert script.read_text() == snapshot(
        """# /// script
# dependencies = []
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.pixi.workspace]
# platforms = []
# ///
print("hello")
"""
    )
    assert not script.with_name("example.py.pixi.lock").exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_tree_reads_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# dependencies = ["requests==2.32.5"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# platforms = ["{CURRENT_PLATFORM}"]
#
# [tool.pixi.dependencies]
# bzip2 = "*"
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    original_script = script.read_text()
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command(
        [
            pixi,
            "tree",
            "--script",
            script,
            "--environment",
            "test",
            "--no-install",
        ],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not support --environment",
            "one implicit default run environment",
        ],
    )
    output = verify_cli_command(
        [pixi, "tree", "--script", script, "--no-install"],
        stdout_contains=["bzip2", "requests"],
    )
    assert "default" not in output.stdout
    assert script.read_text() == original_script
    assert not script_lock.exists()

    verify_cli_command([pixi, "lock", "--script", script])
    original_lock = script_lock.read_text()
    verify_cli_command(
        [pixi, "tree", "--script", script, "--locked", "--no-install"],
        stdout_contains=["bzip2", "requests"],
    )

    assert script.read_text() == original_script
    assert script_lock.read_text() == original_lock
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_list_reads_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# dependencies = ["requests==2.32.5"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# platforms = ["{CURRENT_PLATFORM}"]
#
# [tool.pixi.dependencies]
# bzip2 = "*"
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    original_script = script.read_text()
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command(
        [
            pixi,
            "list",
            "--script",
            script,
            "--environment",
            "test",
            "--no-install",
        ],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not support --environment",
            "one implicit default run environment",
        ],
    )
    verify_cli_command(
        [pixi, "list", "--script", script, "--no-install"],
        stdout_contains=["bzip2", "requests"],
    )
    assert script.read_text() == original_script
    assert not script_lock.exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_workspace_export_reads_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# dependencies = ["requests==2.32.5"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# platforms = ["{CURRENT_PLATFORM}"]
#
# [tool.pixi.dependencies]
# bzip2 = "*"
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    original_script = script.read_text()
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command(
        [
            pixi,
            "workspace",
            "export",
            "conda-environment",
            "--script",
            script,
            "--environment",
            "test",
        ],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not support --environment",
            "one implicit default run environment",
        ],
    )
    environment = verify_cli_command(
        [
            pixi,
            "workspace",
            "export",
            "conda-environment",
            "--script",
            script,
        ],
        stdout_contains=["name: default", "bzip2", "requests==2.32.5"],
    )
    assert environment.stdout == snapshot(
        """\
name: default
channels:
- https://prefix.dev/conda-forge
- nodefaults
dependencies:
- bzip2 *
- python *
- pip
- pip:
  - requests==2.32.5

"""
    )

    export_dir = tmp_pixi_workspace / "explicit"
    export_dir.mkdir()
    verify_cli_command(
        [
            pixi,
            "workspace",
            "export",
            "conda-explicit-spec",
            "--script",
            script,
            "--environment",
            "test",
            "--no-install",
            export_dir,
        ],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not support --environment",
            "one implicit default run environment",
        ],
    )
    verify_cli_command(
        [
            pixi,
            "workspace",
            "export",
            "conda-explicit-spec",
            "--script",
            script,
            "--no-install",
            "--ignore-pypi-errors",
            export_dir,
        ]
    )
    explicit_specs = list(export_dir.glob("*_conda_spec.txt"))
    assert len(explicit_specs) == 1
    assert (
        explicit_specs[0]
        .read_text()
        .startswith(
            f"# Generated by `pixi workspace export`\n# platform: {CURRENT_PLATFORM}\n@EXPLICIT\n"
        )
    )
    assert script.read_text() == original_script
    assert not script_lock.exists()

    verify_cli_command([pixi, "lock", "--script", script])
    original_lock = script_lock.read_text()
    locked_export_dir = tmp_pixi_workspace / "explicit-locked"
    locked_export_dir.mkdir()
    verify_cli_command(
        [
            pixi,
            "workspace",
            "export",
            "conda-explicit-spec",
            "--script",
            script,
            "--locked",
            "--no-install",
            "--ignore-pypi-errors",
            locked_export_dir,
        ]
    )
    assert len(list(locked_export_dir.glob("*_conda_spec.txt"))) == 1
    locked_environment = verify_cli_command(
        [
            pixi,
            "workspace",
            "export",
            "conda-environment",
            "--script",
            script,
            "--from-lock-file",
        ],
        stdout_contains=["bzip2", "requests==2.32.5"],
    )
    assert "name: default" in locked_environment.stdout

    assert script.read_text() == original_script
    assert script_lock.read_text() == original_lock
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


@pytest.mark.slow
def test_pixi_remove_script_uses_explicit_ecosystem(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# dependencies = ["requests==2.32.5"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.pixi.dependencies]
# bzip2 = "*"
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command(
        [pixi, "remove", "--script", script, "--no-install", "bzip2"],
        stderr_contains="Removed bzip2",
    )
    assert not script_lock.exists()

    verify_cli_command([pixi, "lock", "--script", script], cwd=tmp_pixi_workspace)
    original_lock = script_lock.read_text()

    verify_cli_command(
        [pixi, "remove", "--script", script, "--no-install", "--pypi", "requests"],
        stderr_contains="Removed requests",
    )

    assert script.read_text() == snapshot(
        f'''# /// script
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    assert script_lock.read_text() != original_lock
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


# The virtual_packages channel only ships the cuda package for these subdirs.
_CUDA_CHANNEL_SUBDIRS = {"linux-64", "win-64"}

requires_cuda_channel = pytest.mark.skipif(
    CURRENT_PLATFORM not in _CUDA_CHANNEL_SUBDIRS,
    reason="virtual_packages channel ships the cuda package only for linux-64 and win-64",
)


def _script_without_platforms(path: Path, extra_channel: str | None, dependency: str) -> Path:
    """A script with no `platforms`, so pixi picks one for it.

    Every script needs `python`, so conda-forge is always in the list; a test
    channel goes in front of it when the test needs a package from one.
    """
    channels = [f'"{extra_channel}"'] if extra_channel else []
    channels.append(f'"{CONDA_FORGE_CHANNEL}"')
    path.write_text(
        f"""# /// script
# dependencies = []
#
# [tool.pixi.workspace]
# channels = [{", ".join(channels)}]
#
# [tool.pixi.dependencies]
# {dependency}
# ///
print("SCRIPT-RAN")
"""
    )
    return path


@pytest.mark.slow
@requires_cuda_channel
def test_script_without_platforms_solves_against_the_machine(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """A script that declares no platforms is resolved for the machine it runs
    on, not for pixi's per-subdir defaults.

    The in-repo ``cuda`` package needs ``__cuda >=12``, and ``__cuda`` is never
    a subdir default, so it can only resolve if the host's virtual packages
    reached the solver.
    """
    script = _script_without_platforms(
        tmp_pixi_workspace / "gpu.py", virtual_packages_channel, 'cuda = "*"'
    )
    verify_cli_command(
        [pixi, "run", "--script", script],
        ExitCode.SUCCESS,
        env={"CONDA_OVERRIDE_CUDA": "12"},
        stdout_contains="SCRIPT-RAN",
    )


@pytest.mark.slow
@requires_cuda_channel
def test_script_without_platforms_respects_a_machine_below_the_floor(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """The guard rail for the test above: below the package's ``__cuda >=12``
    floor the same script must fail, so the success there cannot come from the
    requirement being ignored."""
    script = _script_without_platforms(
        tmp_pixi_workspace / "gpu.py", virtual_packages_channel, 'cuda = "*"'
    )
    verify_cli_command(
        [pixi, "run", "--script", script],
        ExitCode.FAILURE,
        env={"CONDA_OVERRIDE_CUDA": "10"},
    )


@pytest.mark.slow
@requires_cuda_channel
def test_script_lock_file_round_trips_without_resolving_again(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """``pixi lock --script`` records the host platform, and the next run reuses
    it.

    The lock file's platform has to be read back with the virtual packages it
    was locked with. As a bare subdir the environment would look absent, every
    run would re-resolve, and the lock file would be rewritten with pixi's
    defaults.
    """
    script = _script_without_platforms(
        tmp_pixi_workspace / "gpu.py", virtual_packages_channel, 'cuda = "*"'
    )
    lock = tmp_pixi_workspace / "gpu.py.pixi.lock"
    env = {"CONDA_OVERRIDE_CUDA": "12"}

    verify_cli_command([pixi, "lock", "--script", script], ExitCode.SUCCESS, env=env)
    locked = json.loads(
        verify_cli_command(
            [pixi, "workspace", "platform", "list", "--script", script, "--json"],
            ExitCode.SUCCESS,
            env=env,
        ).stdout
    )
    declared = [row for row in locked["platforms"] if not row.get("is_autodetected")]
    assert declared, locked
    assert any("cuda=12" in row["virtual_packages"] for row in declared), declared

    before = lock.read_text()
    verify_cli_command(
        [pixi, "run", "--script", script, "--locked"],
        ExitCode.SUCCESS,
        env=env,
        stdout_contains="SCRIPT-RAN",
    )
    assert lock.read_text() == before


@pytest.mark.slow
def test_script_frozen_refuses_a_lock_file_without_an_entry_for_this_machine(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """`--frozen` consumes the lock without checking it, so a lock file with no
    row for the platform we run on has to be refused here rather than yielding
    an empty environment that runs against whatever `python` is on `PATH`."""
    script = _script_without_platforms(tmp_pixi_workspace / "plain.py", None, 'python = "3.12.*"')
    other = "win-64" if not CURRENT_PLATFORM.startswith("win") else "linux-64"

    verify_cli_command(
        [pixi, "lock", "--script", script],
        ExitCode.SUCCESS,
        env={"PIXI_OVERRIDE_PLATFORM": other},
    )
    verify_cli_command(
        [pixi, "run", "--script", script, "--frozen"],
        ExitCode.FAILURE,
        stderr_contains="has no entry for platform",
    )


@pytest.mark.slow
def test_lock_script_without_platforms_warns_that_it_records_this_machine(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """``pixi lock --script`` is the only command that persists the picked
    platform, so it says so once rather than leaving a machine-specific lock
    looking portable."""
    script = _script_without_platforms(tmp_pixi_workspace / "plain.py", None, "")
    verify_cli_command(
        [pixi, "lock", "--script", script],
        ExitCode.SUCCESS,
        stderr_contains=[
            "declares no platforms",
            "--auto-detect",
        ],
    )


@pytest.mark.slow
def test_script_with_an_unparsable_lock_file_does_not_blame_the_platform(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A lock file that does not parse says nothing about the platform it was
    locked for, so the platform pick stays quiet and lets the loader report the
    parse error it can point at a line of."""
    script = _script_without_platforms(tmp_pixi_workspace / "plain.py", None, "")
    (tmp_pixi_workspace / "plain.py.pixi.lock").write_text("this is not yaml: [ }\n")

    verify_cli_command(
        [pixi, "run", "--script", script],
        ExitCode.FAILURE,
        stderr_excludes="cannot run",
    )


@pytest.mark.slow
def test_script_warns_before_dropping_foreign_subdirs_from_a_lock_file(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A script that declares no platforms asks for one platform, the one it
    runs on, so rows for other subdirs go. They take their packages with them,
    which is too much of a checked-in lock file to lose in silence."""
    script = tmp_pixi_workspace / "multi.py"
    other = "win-64" if not CURRENT_PLATFORM.startswith("win") else "linux-64"
    script.write_text(
        f"""# /// script
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# platforms = ["{CURRENT_PLATFORM}", "{other}"]
#
# [tool.pixi.dependencies]
# python = "3.12.*"
# ///
print("SCRIPT-RAN")
"""
    )
    verify_cli_command([pixi, "lock", "--script", script], ExitCode.SUCCESS)

    # Dropping the `platforms` line is what leaves the foreign rows behind.
    script.write_text(
        "\n".join(
            line for line in script.read_text().splitlines() if not line.startswith("# platforms")
        )
        + "\n"
    )
    verify_cli_command(
        [pixi, "run", "--script", script],
        ExitCode.SUCCESS,
        stdout_contains="SCRIPT-RAN",
        stderr_contains=[f"'{other}'", "does not ask for"],
    )
