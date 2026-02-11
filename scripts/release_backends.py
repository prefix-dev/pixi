"""Interactive release script for pixi build backends.

Bumps versions in Cargo.toml/pyproject.toml files, creates git tags, and pushes them.
Tag format: {binary-name}-v{version} (e.g., pixi-build-cmake-v0.3.10)

Used by conda-forge feedstocks of the backends.
"""

import atexit
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import questionary  # pyright: ignore[reportMissingImports]
import tomlkit  # pyright: ignore[reportMissingImports]
from rich.console import Console
from rich.prompt import Confirm
from rich.table import Table

UPSTREAM_REPO = "prefix-dev/pixi"

# Each entry needs "binary" and "version_file".  Optional overrides:
#   version_table      – defaults to "package"
#   in_cargo_workspace – defaults to False
BACKEND_DEFS: list[dict[str, Any]] = [
    {
        "binary": "pixi-build-cmake",
        "version_file": "crates/pixi_build_cmake/Cargo.toml",
        "in_cargo_workspace": True,
    },
    {
        "binary": "pixi-build-python",
        "version_file": "crates/pixi_build_python/Cargo.toml",
        "in_cargo_workspace": True,
    },
    {
        "binary": "pixi-build-rust",
        "version_file": "crates/pixi_build_rust/Cargo.toml",
        "in_cargo_workspace": True,
    },
    {
        "binary": "pixi-build-mojo",
        "version_file": "crates/pixi_build_mojo/Cargo.toml",
        "in_cargo_workspace": True,
    },
    {
        "binary": "pixi-build-rattler-build",
        "version_file": "crates/pixi_build_rattler_build/Cargo.toml",
        "in_cargo_workspace": True,
    },
    {
        "binary": "py-pixi-build-backend",
        "version_file": "pixi-build-backends/py-pixi-build-backend/Cargo.toml",
    },
    {
        "binary": "pixi-build-ros",
        "version_file": "pixi-build-backends/backends/pixi-build-ros/pyproject.toml",
        "version_table": "project",
    },
]

STEPS = [
    "Choose version bumps",
    "Apply version bumps and update lockfiles",
    "Run linting",
    "Create and merge PR",
    "Choose backends to tag",
    "Create tags and push",
]

console = Console(stderr=True)

completed: list[str] = []


def print_summary() -> None:
    if completed:
        console.print("\n[bold]Summary of completed steps:[/bold]")
        for step in completed:
            console.print(f"  - {step}")


atexit.register(print_summary)


@dataclass
class Backend:
    binary: str
    version_file: str
    version_table: str = "package"
    in_cargo_workspace: bool = False
    version: str = ""
    new_version: str = ""

    @property
    def cargo_name(self) -> str:
        """Cargo package name for `cargo update --package`."""
        return self.binary.replace("-", "_")

    @property
    def version_path(self) -> Path:
        return Path(self.version_file)

    @property
    def tag(self) -> str:
        return f"{self.binary}-v{self.new_version or self.version}"


def load_backends() -> list[Backend]:
    backends: list[Backend] = []
    for spec in BACKEND_DEFS:
        b = Backend(
            binary=spec["binary"],
            version_file=spec["version_file"],
            version_table=spec.get("version_table", "package"),
            in_cargo_workspace=spec.get("in_cargo_workspace", False),
        )
        b.version = get_version(b.version_path, b.version_table)
        b.new_version = b.version
        backends.append(b)
    return backends


def get_version(path: Path, table: str = "package") -> str:
    doc = tomlkit.parse(path.read_text())  # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]
    version = doc[table]["version"]  # pyright: ignore[reportUnknownVariableType]
    if not isinstance(version, str):
        raise ValueError(f"Could not find version in {path}")
    return version


def bump_version(version: str, bump_type: str) -> str:
    major, minor, patch = (int(x) for x in version.split("."))
    if bump_type == "major":
        return f"{major + 1}.0.0"
    if bump_type == "minor":
        return f"{major}.{minor + 1}.0"
    if bump_type == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise ValueError(f"Unknown bump type: {bump_type}")


def set_version(path: Path, new_version: str, table: str = "package") -> None:
    doc = tomlkit.parse(path.read_text())  # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]
    doc[table]["version"] = new_version
    path.write_text(tomlkit.dumps(doc))  # pyright: ignore[reportUnknownMemberType, reportUnknownArgumentType]


def run(cmd: list[str]) -> None:
    console.print(f"[dim]$ {' '.join(cmd)}[/dim]")
    result = subprocess.run(cmd)
    if result.returncode != 0:
        console.print(f"[bold red]Command failed:[/bold red] {' '.join(cmd)}")
        sys.exit(1)


def find_remote() -> str:
    """Find a git remote that points to prefix-dev/pixi.

    Checks "upstream" first, then "origin", then all others.
    """
    output = subprocess.run(
        ["git", "remote", "--verbose"],
        capture_output=True,
        text=True,
    ).stdout
    remotes: dict[str, str] = {}
    for line in output.splitlines():
        parts = line.split()
        if len(parts) >= 2:
            remotes[parts[0]] = parts[1]
    for name in ["upstream", "origin"]:
        if name in remotes and UPSTREAM_REPO in remotes[name]:
            return name
    for name, url in remotes.items():
        if UPSTREAM_REPO in url:
            return name
    console.print(f"[bold red]No git remote found for {UPSTREAM_REPO}[/bold red]")
    sys.exit(1)


def _ask(question: Any) -> Any:
    """Ask a questionary question, exiting on Ctrl+C."""
    answer = question.ask()
    if answer is None:
        console.print("\n[dim]Interrupted.[/dim]")
        sys.exit(0)
    return answer


def select(message: str, choices: list[str], default: str | None = None) -> str:
    result: str = _ask(questionary.select(message, choices=choices, default=default))  # pyright: ignore[reportUnknownMemberType]
    return result


def checkbox(message: str, backends: list[Backend]) -> list[Backend]:
    choices: list[Any] = [
        questionary.Choice(f"{b.binary} v{b.version}", value=i, checked=True)  # pyright: ignore[reportUnknownMemberType]
        for i, b in enumerate(backends)
    ]
    selected_indices: list[int] = _ask(questionary.checkbox(message, choices=choices))  # pyright: ignore[reportUnknownMemberType]
    return [backends[i] for i in selected_indices]


def show_versions(backends: list[Backend]) -> None:
    table = Table(title="Current Versions")
    table.add_column("Backend", style="cyan")
    table.add_column("Version", style="green")
    for b in backends:
        table.add_row(b.binary, b.version)
    console.print(table)


def main() -> None:
    console.print("\n[bold]Backend Release[/bold]\n")

    remote = find_remote()
    console.print(f"  Using git remote [cyan]{remote}[/cyan]\n")

    backends = load_backends()
    show_versions(backends)
    console.print()

    # Select starting step
    step_choices = [f"{i}. {s}" for i, s in enumerate(STEPS, 1)]
    selected = select("Start from step:", step_choices, default=step_choices[0])
    start_step = int(selected.split(".")[0])

    step = 0
    updated: list[Backend] = []
    to_tag: list[Backend] = []

    try:
        # Step 1: Choose version bumps
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")

            for b in backends:
                choice = select(
                    f"{b.binary} ({b.version}):",
                    ["skip", "patch", "minor", "major"],
                    default="skip",
                )
                if choice != "skip":
                    b.new_version = bump_version(b.version, choice)
                    updated.append(b)
                    console.print(f"  -> [bold green]{b.new_version}[/bold green]")
                else:
                    console.print("  -> [dim]no change[/dim]")

            if not updated:
                console.print("\n[yellow]No versions were bumped.[/yellow]")
                if not Confirm.ask("Continue to tagging anyway?", default=False):
                    return
            else:
                console.print("\n  Planned bumps:")
                for b in updated:
                    console.print(f"    {b.binary}: {b.version} -> [green]{b.new_version}[/green]")

            completed.append("Chose version bumps")

        # Step 2: Apply version bumps and update lockfiles
        step += 1
        if start_step <= step and updated:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")

            for b in updated:
                set_version(b.version_path, b.new_version, b.version_table)
                console.print(f"  Updated {b.version_file}")

            # Update Cargo.lock for workspace backends
            workspace_updated = [b for b in updated if b.in_cargo_workspace]
            if workspace_updated:
                console.print()
                pkgs: list[str] = []
                for b in workspace_updated:
                    pkgs.extend(["--package", b.cargo_name])
                run(["cargo", "update", *pkgs])

            # Update Cargo.lock for py-pixi-build-backend (separate workspace)
            py_backend = next((b for b in updated if b.binary == "py-pixi-build-backend"), None)
            if py_backend:
                console.print()
                run(
                    [
                        "cargo",
                        "update",
                        "--package",
                        py_backend.cargo_name,
                        "--manifest-path",
                        str(py_backend.version_path),
                    ]
                )

            completed.append("Applied version bumps and updated lockfiles")

        # Step 3: Run linting
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")
            run(["pixi", "run", "--environment", "lefthook", "lint-fast"])
            completed.append("Linting passed")

        # Step 4: Create and merge PR
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")
            console.print("  Create a PR with the version bump changes and get it merged.")
            Confirm.ask("PR created and merged?", default=False)
            completed.append("PR created and merged")

        # Step 5: Choose backends to tag
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")

            # Reload versions from disk in case they changed via the merged PR
            backends = load_backends()
            show_versions(backends)
            console.print()

            to_tag = checkbox("Select backends to tag:", backends)

            if not to_tag:
                console.print("[dim]No backends to tag.[/dim]")
                return

            console.print("\n  Tags to create:")
            for b in to_tag:
                console.print(f"    [cyan]{b.tag}[/cyan]")

            console.print()
            if not Confirm.ask("Proceed?", default=True):
                console.print("[dim]Aborted.[/dim]")
                return

            completed.append("Chose backends to tag")

        # Step 6: Create tags and push
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")

            # If resuming directly into this step, ask which backends to tag
            if not to_tag:
                backends = load_backends()
                show_versions(backends)
                console.print()

                to_tag = checkbox("Select backends to tag:", backends)

                if not to_tag:
                    console.print("[dim]No backends to tag.[/dim]")
                    return

            tags = [b.tag for b in to_tag]
            for tag in tags:
                run(["git", "tag", tag])
                console.print(f"  Created [cyan]{tag}[/cyan]")

            console.print(f"\n  Pushing tags to [cyan]{remote}[/cyan]...")
            run(["git", "push", remote, *tags])

            completed.append("Created and pushed tags")

        console.print("\n[bold green]Done![/bold green]")

    except KeyboardInterrupt:
        console.print("\n[dim]Interrupted.[/dim]")


if __name__ == "__main__":
    main()
