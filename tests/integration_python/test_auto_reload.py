import os
import signal
import subprocess
import time
from pathlib import Path
import select

from .common import EMPTY_BOILERPLATE_PROJECT


# Helper function for non-blocking reads with timeout
def read_line_with_timeout(process, timeout=5):
    if process.stdout is None:
        return ""

    # Use select to wait for data with timeout
    ready, _, _ = select.select([process.stdout], [], [], timeout)
    if ready:
        return process.stdout.readline().strip()
    return ""


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

    cmd = [pixi, "watch", "--manifest-path", str(manifest), "watch-test"]
    process = subprocess.Popen(
        [str(c) for c in cmd],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )

    # Wait for process to start
    time.sleep(1)

    # Check initial run
    initial_output_found = False
    for _ in range(10):
        line = read_line_with_timeout(process)
        if line and "initial content" in line:
            initial_output_found = True
            break
        time.sleep(0.3)

    assert initial_output_found, "Task didn't show initial content"

    time.sleep(2)  # Wait for watcher to set up

    # Use a more robust approach to file modification
    # 1. Remove the file first
    if input_file.exists():
        input_file.unlink()
    time.sleep(0.5)

    # 2. Create the file again with new content
    with open(input_file, "w") as f:
        f.write("updated content")
        f.flush()
        os.fsync(f.fileno())

    # 3. Make additional minor changes to ensure the watcher detects it
    for i in range(3):
        time.sleep(0.5)
        with open(input_file, "a") as f:
            f.write(" ")  # Add a space to change file
            f.flush()
            os.fsync(f.fileno())

    # Check for rerun after file modification
    rerun_output_found = False
    for _ in range(20):
        line = read_line_with_timeout(process, timeout=6)
        if line and "updated content" in line:
            rerun_output_found = True
            break
        time.sleep(0.5)

    assert rerun_output_found, "Task didn't rerun after file was modified"
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

    # Wait for initial output
    initial_output_found = False
    for _ in range(10):
        line = read_line_with_timeout(process)
        if line and "f1=one" in line and "f2=two" in line:
            initial_output_found = True
            break
        time.sleep(0.3)
    assert initial_output_found, "Task didn't show initial content from both files"

    time.sleep(2)  # Wait for watcher to set up

    # Use a more robust approach to file modification
    # 1. Remove the file first
    if file1.exists():
        file1.unlink()
    time.sleep(0.5)

    # 2. Create the file again with new content
    with open(file1, "w") as f:
        f.write("one-updated")
        f.flush()
        os.fsync(f.fileno())

    # 3. Make additional minor changes to ensure the watcher detects it
    for i in range(3):
        time.sleep(0.5)
        with open(file1, "a") as f:
            f.write(" ")  # Add a space to change file
            f.flush()
            os.fsync(f.fileno())

    # Try more times with longer waits
    rerun1_found = False
    for _ in range(20):
        line = read_line_with_timeout(process, timeout=6)
        if line and "one-updated" in line and "f2=two" in line:
            rerun1_found = True
            break
        time.sleep(0.5)

    assert rerun1_found, "Task didn't rerun after file1 was modified"
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
    """Test behavior with empty watched-files list (should run once and exit automatically)."""
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    watch-empty = {{ cmd = "echo Empty watched files", inputs = [] }}
    """
    manifest.write_text(toml)

    cmd = [str(pixi), "watch", "--manifest-path", str(manifest), "watch-empty"]
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=str(tmp_pixi_workspace),
    )

    # Check if the task runs at least once
    task_executed = False
    for _ in range(10):
        line = read_line_with_timeout(process)
        if line and "Empty watched files" in line:
            task_executed = True
            break
        time.sleep(0.3)

    assert task_executed, "Task didn't execute"

    for _ in range(10):
        if process.poll() is not None:
            break
        time.sleep(0.5)

    assert (
        process.poll() is not None
    ), "Process should have exited on its own with empty inputs list"

    if process.poll() is None:
        terminate_process(process, 1)
