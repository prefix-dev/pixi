"""Interactive release script for pixi build backends.

Bumps versions in Cargo.toml/pyproject.toml files, creates git tags, and pushes them.
Tag format: {binary-name}-v{version} (e.g., pixi-build-cmake-v0.3.10)

Used by conda-forge feedstocks of the backends.
"""

import atexit
import hashlib
import subprocess
import sys
import tempfile
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import questionary  # pyright: ignore[reportMissingImports]
import tomlkit  # pyright: ignore[reportMissingImports]
from rich.console import Console
from rich.prompt import Confirm
from rich.table import Table
from ruamel.yaml import YAML  # pyright: ignore[reportMissingImports,reportUnknownVariableType]

UPSTREAM_REPO = "prefix-dev/pixi"
USE_JJ = Path(".jj").is_dir()

# Each entry needs "binary" and "version_file".  Optional overrides:
#   version_table      – defaults to "package"
#   in_cargo_workspace – defaults to False
BACKEND_DEFS: list[dict[str, Any]] = [
    {
        "binary": "pixi-build-cmake",
        "version_file": "crates/pixi_build_cmake/Cargo.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-cmake-feedstock",
    },
    {
        "binary": "pixi-build-python",
        "version_file": "crates/pixi_build_python/Cargo.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-python-feedstock",
    },
    {
        "binary": "pixi-build-rust",
        "version_file": "crates/pixi_build_rust/Cargo.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-rust-feedstock",
    },
    {
        "binary": "pixi-build-mojo",
        "version_file": "crates/pixi_build_mojo/Cargo.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-mojo-feedstock",
    },
    {
        "binary": "pixi-build-rattler-build",
        "version_file": "crates/pixi_build_rattler_build/Cargo.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-rattler-build-feedstock",
    },
    {
        "binary": "py-pixi-build-backend",
        "version_file": "pixi-build-backends/py-pixi-build-backend/Cargo.toml",
        "feedstock": "conda-forge/py-pixi-build-backend-feedstock",
    },
    {
        "binary": "pixi-build-ros",
        "version_file": "pixi-build-backends/backends/pixi-build-ros/pyproject.toml",
        "version_table": "project",
        "feedstock": "conda-forge/pixi-build-ros-feedstock",
    },
]

STEPS = [
    "Choose version bumps",
    "Apply version bumps and update lockfiles",
    "Run linting",
    "Commit and push changes",
    "Create and merge PR",
    "Choose backends to tag",
    "Create tags and push",
    "Update conda-forge feedstocks",
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
    feedstock: str
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
            feedstock=spec["feedstock"],
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


def commit_and_push(remote: str, branch: str, message: str) -> None:
    """Create a branch, commit changes, and push to the remote."""
    if USE_JJ:
        run(["jj", "describe", "--message", message])
        # Track the remote bookmark if it exists, so set + push can update it
        subprocess.run(
            ["jj", "bookmark", "track", branch, f"--remote={remote}"],
            capture_output=True,
        )
        run(["jj", "bookmark", "set", "--allow-backwards", branch])
        run(["jj", "git", "push", "--remote", remote, "--bookmark", branch])
        run(["jj", "new"])
    else:
        # Delete the branch if it already exists locally
        result = subprocess.run(
            ["git", "branch", "--list", branch],
            capture_output=True,
            text=True,
        )
        if result.stdout.strip():
            run(["git", "branch", "--delete", branch])
        run(["git", "switch", "--create", branch])
        run(["git", "commit", "--all", "--message", message])
        run(["git", "push", "--set-upstream", remote, branch])


def sync_to_main(remote: str) -> None:
    """Fetch latest changes and switch to the up-to-date main branch."""
    if USE_JJ:
        run(["jj", "git", "fetch", "--remote", remote])
        run(["jj", "new", "main"])
    else:
        run(["git", "checkout", "main"])
        run(["git", "pull", remote, "main"])


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


def get_latest_tag_version(binary: str) -> str | None:
    """Find the latest git tag for a backend and return the version part."""
    result = subprocess.run(
        ["git", "tag", "--list", f"{binary}-v*", "--sort=-v:refname"],
        capture_output=True,
        text=True,
    )
    tags = result.stdout.strip().splitlines()
    if not tags:
        return None
    # Tag format: {binary}-v{version}
    return tags[0].removeprefix(f"{binary}-v")


def get_feedstock_version(clone_dir: Path) -> str:
    """Read the version from a feedstock's recipe.yaml."""
    yaml = YAML()  # pyright: ignore[reportUnknownVariableType]
    data = yaml.load(clone_dir / "recipe" / "recipe.yaml")  # pyright: ignore[reportUnknownVariableType,reportUnknownMemberType]
    version: str = data["context"]["version"]  # pyright: ignore[reportUnknownVariableType]
    return version  # pyright: ignore[reportUnknownVariableType]


def compute_tarball_sha256(tag: str) -> str:
    """Download the source tarball for a tag and return its SHA-256 hash."""
    url = f"https://github.com/{UPSTREAM_REPO}/archive/refs/tags/{tag}.tar.gz"
    console.print(f"  Downloading [dim]{url}[/dim]")
    sha256 = hashlib.sha256()
    with urllib.request.urlopen(url) as response:  # noqa: S310
        while chunk := response.read(65536):
            sha256.update(chunk)
    digest = sha256.hexdigest()
    console.print(f"  SHA-256: [cyan]{digest}[/cyan]")
    return digest


def update_feedstock_recipe(recipe_path: Path, new_version: str, sha256: str) -> None:
    """Update version, sha256, and build number in a feedstock recipe."""
    yaml = YAML()  # pyright: ignore[reportUnknownVariableType]
    yaml.preserve_quotes = True
    data = yaml.load(recipe_path)  # pyright: ignore[reportUnknownVariableType,reportUnknownMemberType]
    data["context"]["version"] = new_version
    data["source"]["sha256"] = sha256
    data["build"]["number"] = 0
    yaml.dump(data, recipe_path)  # pyright: ignore[reportUnknownMemberType]


def run_in_dir(cmd: list[str], cwd: Path) -> None:
    """Run a command in a specific directory."""
    console.print(f"[dim]$ {' '.join(cmd)}  (in {cwd})[/dim]")
    result = subprocess.run(cmd, cwd=cwd)
    if result.returncode != 0:
        console.print(f"[bold red]Command failed:[/bold red] {' '.join(cmd)}")
        sys.exit(1)


def update_feedstock(backend: Backend, clone_dir: Path) -> None:
    """Update recipe, rerender, and open a PR for an already-cloned feedstock."""
    feedstock = backend.feedstock
    tag = backend.tag
    version = backend.new_version or backend.version
    branch = f"v{version}"

    console.print(f"\n  [bold]{feedstock}[/bold]")

    # Create branch
    run_in_dir(["git", "checkout", "-b", branch], cwd=clone_dir)

    # Compute sha256
    sha256 = compute_tarball_sha256(tag)

    # Update recipe
    recipe_path = clone_dir / "recipe" / "recipe.yaml"
    update_feedstock_recipe(recipe_path, version, sha256)
    console.print(f"  Updated {recipe_path.name}")

    # Rerender
    run_in_dir(["pixi", "exec", "conda-smithy", "rerender"], cwd=clone_dir)

    # Commit and push
    run_in_dir(["git", "add", "--all"], cwd=clone_dir)
    run_in_dir(
        ["git", "commit", "--message", f"Update to {backend.binary} v{version}"],
        cwd=clone_dir,
    )
    run_in_dir(["git", "push", "--set-upstream", "--force", "origin", branch], cwd=clone_dir)

    # Create PR
    run_in_dir(
        [
            "gh",
            "pr",
            "create",
            "--repo",
            feedstock,
            "--title",
            f"Update to {backend.binary} v{version}",
            "--body",
            f"Update {backend.binary} to v{version}.\n\nThis PR was automatically created by the pixi backend release script.",
        ],
        cwd=clone_dir,
    )

    console.print(f"  [green]PR created for {feedstock}[/green]")


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
                        py_backend.binary,
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

        # Step 4: Commit and push changes
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")
            branch = "bump/backends-release"
            commit_and_push(remote, branch, "chore: bump backend versions")
            completed.append(f"Committed and pushed to {branch}")

        # Step 5: Create and merge PR
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")
            console.print("  Create a PR with the version bump changes and get it merged.")
            Confirm.ask("PR created and merged?", default=False)
            completed.append("PR created and merged")

        # Step 6: Choose backends to tag
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")

            # Sync to main so tags are created on the merged commit
            sync_to_main(remote)

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

        # Step 7: Create tags and push
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

        # Step 8: Update conda-forge feedstocks
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")

            backends = load_backends()

            # Clone all feedstocks and find which ones are outdated
            with tempfile.TemporaryDirectory() as tmpdir:
                tmp = Path(tmpdir)
                outdated: list[tuple[Backend, Path]] = []

                for b in backends:
                    tag_version = get_latest_tag_version(b.binary)
                    if tag_version is None:
                        console.print(f"  [dim]{b.binary}: no tags found, skipping[/dim]")
                        continue

                    clone_dir = tmp / b.feedstock.split("/")[1]
                    run_in_dir(
                        ["gh", "repo", "fork", b.feedstock, "--clone", "--default-branch-only"],
                        cwd=tmp,
                    )

                    # Sync fork to upstream so our branch is based on the latest state
                    run_in_dir(["gh", "repo", "sync", "--force"], cwd=clone_dir)
                    run_in_dir(["git", "pull", "--ff-only"], cwd=clone_dir)

                    feedstock_version = get_feedstock_version(clone_dir)
                    if feedstock_version == tag_version:
                        console.print(
                            f"  [dim]{b.binary}: feedstock already at v{tag_version}[/dim]"
                        )
                        continue

                    console.print(
                        f"  [yellow]{b.binary}[/yellow]: feedstock v{feedstock_version} -> v{tag_version}"
                    )
                    b.new_version = tag_version
                    outdated.append((b, clone_dir))

                if not outdated:
                    console.print("\n  [dim]All feedstocks are up to date.[/dim]")
                else:
                    console.print("\n  PRs to open:")
                    for b, _ in outdated:
                        console.print(f"    [cyan]{b.feedstock}[/cyan] -> v{b.new_version}")

                    console.print()
                    if Confirm.ask("Proceed?", default=True):
                        for b, clone_dir in outdated:
                            update_feedstock(b, clone_dir)
                        completed.append("Updated conda-forge feedstocks")
                    else:
                        console.print("[dim]Skipped feedstock updates.[/dim]")

        console.print("\n[bold green]Done![/bold green]")

    except KeyboardInterrupt:
        console.print("\n[dim]Interrupted.[/dim]")


if __name__ == "__main__":
    main()
