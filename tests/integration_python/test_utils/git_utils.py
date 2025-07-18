import shutil
import socket
import subprocess
import time
from pathlib import Path
from typing import Optional, Any


def find_available_port(start_port: int = 9418) -> int:
    """Find an available port starting from the given port."""
    for port in range(start_port, start_port + 100):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            try:
                s.bind(("localhost", port))
                return port
            except OSError:
                continue
    raise RuntimeError("Could not find available port")


class GitTestRepo:
    """A simple git repository for testing that can be served via git daemon."""

    def __init__(self, source_dir: Path, repo_name: str, temp_dir: Path):
        self.source_dir = source_dir
        self.repo_name = repo_name
        self.temp_dir = temp_dir
        self.bare_repo_path: Optional[Path] = None
        self.daemon_process: Optional[subprocess.Popen[Any]] = None
        self.port: Optional[int] = None
        self.git_url: Optional[str] = None

    def create_bare_repo(self) -> None:
        """Create a bare git repository from the source directory in a temp directory."""
        # Copy source directory to temp
        temp_source = self.temp_dir / self.repo_name
        shutil.copytree(self.source_dir, temp_source)

        # Initialize git repository in the copied source
        subprocess.run(
            ["git", "init"],
            cwd=temp_source,
            check=True,
            capture_output=True,
        )

        # Add all files and commit
        subprocess.run(
            ["git", "add", "."],
            cwd=temp_source,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "Initial commit"],
            cwd=temp_source,
            check=True,
            capture_output=True,
        )

    def start_daemon(self) -> str:
        """Start git daemon to serve the repository. Returns the git URL."""
        # Find available port
        self.port = find_available_port()

        # Start git daemon
        cmd = [
            "git",
            "daemon",
            f"--base-path={self.temp_dir}",
            "--reuseaddr",
            "--export-all",
            "--informative-errors",
            f"--port={self.port}",
            "--verbose",
        ]

        print("".join(cmd))
        # Run daemon in background
        self.daemon_process = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

        # Wait a bit for daemon to start
        time.sleep(1.0)

        # Check if process is still running
        if self.daemon_process.poll() is not None:
            stdout, stderr = self.daemon_process.communicate()
            raise RuntimeError(f"Git daemon failed to start: {stderr.decode()}")

        # Test if daemon is responding by trying to connect
        try:
            import socket

            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(2.0)
            result = sock.connect_ex(("localhost", self.port))
            sock.close()
            if result != 0:
                raise RuntimeError(f"Git daemon not responding on port {self.port}")
        except Exception as e:
            raise RuntimeError(f"Failed to connect to git daemon: {e}")

        self.git_url = f"git://localhost:{self.port}/{self.repo_name}"
        return self.git_url

    def stop_daemon(self) -> None:
        """Stop the git daemon."""
        if self.daemon_process and self.daemon_process.poll() is None:
            self.daemon_process.terminate()
            try:
                self.daemon_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.daemon_process.kill()
                self.daemon_process.wait()

    def get_git_url(self) -> str:
        """Get the git URL for cloning."""
        if self.git_url is None:
            raise RuntimeError("Daemon not started")
        return self.git_url

    def cleanup(self) -> None:
        """Clean up daemon and temporary files."""
        self.stop_daemon()

    def __enter__(self) -> "GitTestRepo":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.cleanup()
