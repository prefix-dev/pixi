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
    watch-test = {{ cmd = "echo Running with content: $(cat input.txt)", watched-files = ["input.txt"] }}
    """
    manifest.write_text(toml)

    input_file = tmp_pixi_workspace.joinpath("input.txt")
    input_file.write_text("initial content")

    cmd = [pixi, "run", "--manifest-path", str(manifest), "watch-test"]
    process = subprocess.Popen(
        [str(c) for c in cmd],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )

    initial_output_found = False
    # Check if stdout is None before trying to read from it
    for _ in range(10):
        if process.stdout is None:
            time.sleep(0.3)
            continue

        line = process.stdout.readline().strip()
        if line and "initial content" in line:
            initial_output_found = True
            break
        time.sleep(0.3)

    assert initial_output_found, "Task didn't show initial content"

    time.sleep(1)  # Wait for watcher to set up

    input_file.write_text("updated content")

    rerun_output_found = False
    for _ in range(10):
        if process.stdout is None:
            time.sleep(0.3)
            continue

        line = process.stdout.readline().strip()
        if line and "updated content" in line:
            rerun_output_found = True
            break
        time.sleep(0.3)

    assert rerun_output_found, "Task didn't rerun after file was modified"
    terminate_process(process, 1)


def test_multiple_files_watching(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-multiple = {{ cmd = "echo FILES: f1=$(cat file1.txt) f2=$(cat file2.txt)", watched-files = ["file1.txt", "file2.txt"] }}
    """
    manifest.write_text(toml)

    file1 = tmp_pixi_workspace.joinpath("file1.txt")
    file1.write_text("one")

    file2 = tmp_pixi_workspace.joinpath("file2.txt")
    file2.write_text("two")
    cmd = [str(pixi), "run", "--manifest-path", str(manifest), "watch-multiple"]
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )

    # Wait for initial output
    initial_output_found = False
    for _ in range(10):
        if process.stdout is None:
            time.sleep(0.3)
            continue

        line = process.stdout.readline().strip()
        if line and "f1=one" in line and "f2=two" in line:
            initial_output_found = True
            break
        time.sleep(0.3)
    assert initial_output_found, "Task didn't show initial content from both files"

    time.sleep(1)  # Wait for watcher to set up

    # Modify first file and verify rerun
    file1.write_text("one-updated")

    rerun1_found = False
    for _ in range(10):
        if process.stdout is None:
            time.sleep(0.3)
            continue

        line = process.stdout.readline().strip()
        if line and "f1=one-updated" in line and "f2=two" in line:
            rerun1_found = True
            break
        time.sleep(0.3)

    assert rerun1_found, "Task didn't rerun after file1 was modified"
    terminate_process(process, 2)


def test_glob_pattern_watching(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-glob = {{ cmd = "echo LOG_CONTENT=$(cat test.log)", watched-files = ["*.log"] }}
    """
    manifest.write_text(toml)

    log_file = tmp_pixi_workspace.joinpath("test.log")
    log_file.write_text("initial_data")

    cmd = [str(pixi), "run", "--manifest-path", str(manifest), "watch-glob"]

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
    """Test behavior with empty watched-files list (should run once and exit)."""
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-empty = {{ cmd = "echo Empty watched files", watched-files = [] }}
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
    watch-nonexistent = {{ cmd = "echo File created", watched-files = ["does_not_exist_yet.txt"] }}
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
    initial_run = False
    for _ in range(5):
        if process.stdout is None:
            time.sleep(0.3)
            continue

        line = process.stdout.readline().strip()
        if line and "File created" in line:
            initial_run = True
            break
        time.sleep(0.3)

    assert initial_run, "Task didn't run initially"

    time.sleep(1)  # Wait for watcher to set up

    nonexistent_file = tmp_pixi_workspace.joinpath("does_not_exist_yet.txt")
    nonexistent_file.write_text("now I exist")

    terminate_process(process, 1)
