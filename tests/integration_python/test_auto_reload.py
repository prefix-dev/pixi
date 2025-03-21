import select
import os
import signal
import subprocess
import time
from pathlib import Path
from .common import EMPTY_BOILERPLATE_PROJECT


# Cross-platform process termination function
def terminate_process(process: subprocess.Popen, number_of_tasks: int) -> None:
    """Terminate a process in a cross-platform way."""
    if os.name == "nt":  # Windows
        # On Windows, we can use terminate() which sends Ctrl+C
        process.terminate()
    else:  # Unix (Linux, macOS)
        # On Unix, we can use SIGINT directly
        process.send_signal(signal.SIGINT)
    time.sleep(0.5)
    if process.poll() is None:
        for _ in range(number_of_tasks):
            process.kill()
            process.wait(timeout=1)


def test_file_watching_and_rerunning(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-test = {{ cmd = "echo Running with content: $(cat input.txt)", inputs = ["input.txt"] }}
    """
    manifest.write_text(toml)
    input_file = tmp_pixi_workspace.joinpath("input.txt")
    input_file.write_text("initial content")
    cmd = [str(pixi), "watch", "--manifest-path", str(manifest), "watch-test"]
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )

    ready = False
    while not ready:
        ready, _, _ = select.select([process.stdout], [], [], 0.5)
    line = process.stdout.readline().strip()
    assert "initial content" in line, "Task didn't show initial content"

    input_file.write_text("updated content")

    ready = False
    while not ready:
        ready, _, _ = select.select([process.stdout], [], [], 0.5)
    line = process.stdout.readline().strip()
    assert "updated content" in line, "Task didn't show updated content"
    terminate_process(process, 1)


def test_multiple_files_watching(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-multiple = {{ cmd = "echo FILES: f1=$(cat file1.txt) f2=$(cat file2.txt)", inputs = ["file1.txt", "file2.txt"] }}
    """
    manifest.write_text(toml)
    file1 = tmp_pixi_workspace.joinpath("file1.txt")
    file1.write_text("one")
    file2 = tmp_pixi_workspace.joinpath("file2.txt")
    file2.write_text("two")
    cmd = [str(pixi), "watch", "--manifest-path", str(manifest), "watch-multiple"]
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )

    ready = False
    while not ready:
        ready, _, _ = select.select([process.stdout], [], [], 0.5)
    line = process.stdout.readline().strip()
    assert "f1=one" in line and "f2=two" in line, "Task didn't show initial content from both files"

    file1.write_text("one-updated")
    ready = False
    while not ready:
        ready, _, _ = select.select([process.stdout], [], [], 0.5)
    line = process.stdout.readline().strip()
    assert "f1=one-updated" in line and "f2=two" in line, (
        "Task didn't show updated content from file1"
    )
    terminate_process(process, 2)


def test_glob_pattern_watching(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-glob = {{ cmd = "echo LOG_CONTENT=$(cat test.log)", inputs = ["*.log"] }}
    """
    manifest.write_text(toml)
    log_file = tmp_pixi_workspace.joinpath("test.log")
    log_file.write_text("initial_data")
    cmd = [str(pixi), "watch", "--manifest-path", str(manifest), "watch-glob"]
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )

    time.sleep(1)  # Wait for process to start

    log_file.write_text("modified_data")

    time.sleep(1)  # Wait for change to be detected

    stdout, stderr = process.communicate(timeout=2)
    assert "initial_data" in stdout or "modified_data" in stdout, "Expected output not found"

    terminate_process(process, 1)


def test_empty_watched_files(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test behavior with empty inputs list (should run once and exit)."""
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-empty = {{ cmd = "echo Empty watched files", inputs = [] }}
    """
    manifest.write_text(toml)
    cmd = [str(pixi), "run", "--manifest-path", str(manifest), "watch-empty"]
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )
    stdout, stderr = process.communicate(timeout=3)
    assert "Empty watched files" in stdout, "Task didn't execute"
    if process.poll() is None:
        process.kill()
        process.wait(timeout=1)


def test_nonexistent_watched_file(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-nonexistent = {{ cmd = "echo File created", inputs = ["does_not_exist_yet.txt"] }}
    """
    manifest.write_text(toml)
    cmd = [str(pixi), "run", "--manifest-path", str(manifest), "watch-nonexistent"]
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )
    # Wait for initial run
    ready = False
    while not ready:
        ready, _, _ = select.select([process.stdout], [], [], 0.5)
    line = process.stdout.readline().strip()
    assert "File created" in line, "Task didn't run initially"
    nonexistent_file = tmp_pixi_workspace.joinpath("does_not_exist_yet.txt")
    nonexistent_file.write_text("now I exist")
    terminate_process(process, 1)
