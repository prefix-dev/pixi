import shutil
import subprocess
from pathlib import Path
from typing import Any


class GitTestRepo:
    """A simple git repository for testing that can be cloned directly from a directory path."""

    def __init__(self, source_dir: Path, repo_name: str, temp_dir: Path):
        self.source_dir = source_dir
        self.repo_name = repo_name
        self.temp_dir = temp_dir
        self.repo_path: Path = temp_dir / repo_name

    def create_bare_repo(self) -> None:
        """Create a git repository from the source directory in a temp directory."""
        # Copy source directory to temp
        shutil.copytree(self.source_dir, self.repo_path)

        # Initialize git repository in the copied source
        subprocess.run(
            ["git", "init"],
            cwd=self.repo_path,
            check=True,
            capture_output=True,
        )

        # Add all files and commit
        subprocess.run(
            ["git", "add", "."],
            cwd=self.repo_path,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-m", "Initial commit"],
            cwd=self.repo_path,
            check=True,
            capture_output=True,
        )

    def get_git_url(self) -> str:
        """Get the git URL for cloning (local directory path)."""
        return f"file://{self.repo_path}"

    def cleanup(self) -> None:
        """Clean up temporary files if they exist and we're not using pytest temp_dir."""
        if self.repo_path.exists() and not str(self.temp_dir).startswith("/tmp/pytest-"):
            self.repo_path.unlink(missing_ok=True)

    def __enter__(self) -> "GitTestRepo":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.cleanup()
