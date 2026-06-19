"""Release pixi build backends.

    pixi run release-backends

Asks which step to run, split by the human-merge gate:

    Step 1: bump      # bump versions on a branch, open a PR
    Step 2: publish   # after the PR merges: tag + feedstocks

(The step can also be passed as an argument - bump or publish - to skip the
prompt.)

Both always fetch prefix-dev/pixi by URL and operate on the canonical main, so
they behave identically in a plain git clone or a colocated jj repo, regardless
of which branch (or detached HEAD) you happen to be on.

`bump` branches from the freshly fetched main, lets you pick per-backend
version bumps, reconciles the pixi-build-api-version requirement across the
local sites, and opens a PR. `publish` reads the versions on main, tags the
ones that have no tag yet, pushes the tags, and opens conda-forge feedstock
PRs for any feedstock that lags behind. Because `publish` only tags versions
that are actually on main, it is a clean no-op until the bump PR has merged.

Tag format: {binary-name}-v{version} (e.g. pixi-build-cmake-v0.3.10)
"""

import argparse
import hashlib
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import questionary
import tomlkit
from rattler import MatchSpec, VersionSpec
from rattler.exceptions import InvalidVersionSpecError
from rich.console import Console
from rich.prompt import Confirm
from rich.table import Table
from ruamel.yaml import YAML

ROOT = Path(__file__).resolve().parent.parent
REPO = "prefix-dev/pixi"
REMOTE_URL = f"https://github.com/{REPO}.git"
BUMP_BRANCH = "bump/backends-release"

# Name of the protocol package whose requirement range is kept identical across
# every backend declaration site.
API_PKG = "pixi-build-api-version"

# The all-backends recipe declares per-output run requirements and is also the
# read-only source of truth for context.api_version (the version of the
# pixi-build-api-version package itself).
ALL_BACKENDS_RECIPE = ROOT / "pixi-build-backends/recipe/all-backends/recipe.yaml"

# Each entry needs "binary", "version_file", "pixi_manifest", and "feedstock".
# Paths are repo-relative so they can be read both from disk and via `git show`.
# Optional overrides:
#   version_table      – defaults to "package"
#   in_cargo_workspace – defaults to False
BACKEND_DEFS: list[dict[str, Any]] = [
    {
        "binary": "pixi-build-cmake",
        "version_file": "crates/pixi_build_cmake/Cargo.toml",
        "pixi_manifest": "crates/pixi_build_cmake/pixi.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-cmake-feedstock",
    },
    {
        "binary": "pixi-build-python",
        "version_file": "crates/pixi_build_python/Cargo.toml",
        "pixi_manifest": "crates/pixi_build_python/pixi.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-python-feedstock",
    },
    {
        "binary": "pixi-build-rust",
        "version_file": "crates/pixi_build_rust/Cargo.toml",
        "pixi_manifest": "crates/pixi_build_rust/pixi.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-rust-feedstock",
    },
    {
        "binary": "pixi-build-mojo",
        "version_file": "crates/pixi_build_mojo/Cargo.toml",
        "pixi_manifest": "crates/pixi_build_mojo/pixi.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-mojo-feedstock",
    },
    {
        "binary": "pixi-build-rattler-build",
        "version_file": "crates/pixi_build_rattler_build/Cargo.toml",
        "pixi_manifest": "crates/pixi_build_rattler_build/pixi.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-rattler-build-feedstock",
    },
    {
        "binary": "pixi-build-ros",
        "version_file": "crates/pixi_build_ros/Cargo.toml",
        "pixi_manifest": "crates/pixi_build_ros/pixi.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-ros-feedstock",
    },
    {
        "binary": "pixi-build-r",
        "version_file": "crates/pixi_build_r/Cargo.toml",
        "pixi_manifest": "crates/pixi_build_r/pixi.toml",
        "in_cargo_workspace": True,
        "feedstock": "conda-forge/pixi-build-r-feedstock",
    },
]

console = Console(stderr=True)


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
    def version_path(self) -> Path:
        return ROOT / self.version_file

    @property
    def tag(self) -> str:
        return f"{self.binary}-v{self.new_version or self.version}"


def fail(msg: str) -> None:
    console.print(f"\n[bold red]error:[/bold red] {msg}")
    sys.exit(1)


def run(cmd: list[str], *, cwd: Path = ROOT) -> None:
    location = "" if cwd == ROOT else f"  (in {cwd})"
    console.print(f"[dim]$ {' '.join(cmd)}{location}[/dim]")
    if subprocess.run(cmd, cwd=cwd).returncode != 0:
        fail(f"command failed: {' '.join(cmd)}")


def git_out(*args: str, cwd: Path = ROOT) -> str:
    return subprocess.run(["git", *args], cwd=cwd, text=True, capture_output=True).stdout.strip()


def capture(cmd: list[str]) -> str:
    result = subprocess.run(cmd, cwd=ROOT, text=True, capture_output=True)
    if result.returncode != 0:
        fail(f"command failed: {' '.join(cmd)}\n{result.stderr.strip()}")
    return result.stdout.strip()


def fork_target() -> tuple[str, str]:
    """Return (login, git push target) for the gh user's fork.

    Prefers an existing git remote that points at the fork, so the user's
    configured protocol (SSH or HTTPS) is preserved, and only falls back to a
    constructed HTTPS URL when no such remote is set up.
    """
    login = capture(["gh", "api", "user", "--jq", ".login"])
    slug = f"{login}/{REPO.split('/')[1]}"
    for line in git_out("remote", "-v").splitlines():
        parts = line.split()
        if len(parts) >= 2 and slug in parts[1].replace(":", "/"):
            return login, parts[0]
    return login, f"https://github.com/{slug}.git"


def sync_jj() -> None:
    """Import the git refs created by this script into a colocated jj repo."""
    if not (ROOT / ".jj").is_dir() or shutil.which("jj") is None:
        return
    console.print("\n  Importing git refs into jj...")
    run(["jj", "git", "import"])


# --- Version helpers ---


def version_from_toml(text: str, table: str = "package") -> str:
    doc = tomlkit.parse(text)
    version = doc[table]["version"]
    if not isinstance(version, str):
        raise ValueError("version is not a string")
    return version


def get_version(path: Path, table: str = "package") -> str:
    return version_from_toml(path.read_text(), table)


def bump_version(version: str, bump_type: str) -> str:
    major, minor, patch = (int(x) for x in version.split("."))
    if bump_type == "major":
        return f"{major + 1}.0.0"
    if bump_type == "minor":
        return f"{major}.{minor + 1}.0"
    if bump_type == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise ValueError(f"unknown bump type: {bump_type}")


def set_version(path: Path, new_version: str, table: str = "package") -> None:
    doc = tomlkit.parse(path.read_text())
    doc[table]["version"] = new_version
    path.write_text(tomlkit.dumps(doc))


def load_backends(ref: str | None = None) -> list[Backend]:
    """Build the backend list, reading versions from disk or from a git ref."""
    backends: list[Backend] = []
    for spec in BACKEND_DEFS:
        b = Backend(
            binary=spec["binary"],
            version_file=spec["version_file"],
            feedstock=spec["feedstock"],
            version_table=spec.get("version_table", "package"),
            in_cargo_workspace=spec.get("in_cargo_workspace", False),
        )
        if ref is None:
            b.version = get_version(b.version_path, b.version_table)
        else:
            b.version = version_from_toml(
                git_out("show", f"{ref}:{b.version_file}"), b.version_table
            )
        b.new_version = b.version
        backends.append(b)
    return backends


def show_versions(backends: list[Backend], title: str = "Current Versions") -> None:
    table = Table(title=title)
    table.add_column("Backend", style="cyan")
    table.add_column("Version", style="green")
    for b in backends:
        table.add_row(b.binary, b.version)
    console.print(table)


# --- api-version requirement ---


def parse_api_requirement(entry: str) -> str | None:
    """Extract the spec from a "pixi-build-api-version <spec>" run requirement.

    Returns the spec portion (e.g. ">=5,<6"), an empty string when the package is
    listed without a spec, or None when the entry is for another package.
    """
    spec = MatchSpec(entry)
    if str(spec.name.normalized) != API_PKG:
        return None
    return str(spec.version) if spec.version is not None else ""


def validate_spec(spec: str) -> str | None:
    """Return an error message when spec does not parse as a version spec."""
    try:
        VersionSpec(spec)
    except InvalidVersionSpecError as exc:
        return f"not a valid version spec: {exc}"
    return None


def read_crate_specs() -> dict[str, tuple[Path, str]]:
    """Map backend binary name to its crate pixi.toml path and api-version spec."""
    specs: dict[str, tuple[Path, str]] = {}
    for spec in BACKEND_DEFS:
        path = ROOT / spec["pixi_manifest"]
        doc: Any = tomlkit.parse(path.read_text())
        specs[spec["binary"]] = (path, str(doc["package"]["run-dependencies"][API_PKG]))
    return specs


def set_crate_spec(path: Path, target: str) -> None:
    """Write the api-version run-dependency in a crate pixi.toml."""
    doc = tomlkit.parse(path.read_text())
    doc["package"]["run-dependencies"][API_PKG] = target
    path.write_text(tomlkit.dumps(doc))


def discover_recipe_specs(data: Any) -> dict[str, str]:
    """Map backend binary name to its api-version spec in the all-backends recipe.

    Skips the staging output and the pixi-build-api-version package output itself.
    """
    specs: dict[str, str] = {}
    for output in data.get("outputs", []):
        package = output.get("package")
        if not package:
            continue
        name = package.get("name")
        if name == API_PKG:
            continue
        run_reqs = output.get("requirements", {}).get("run")
        if not run_reqs:
            continue
        for entry in run_reqs:
            spec = parse_api_requirement(str(entry))
            if spec is not None:
                specs[name] = spec
                break
    return specs


def rewrite_run_api_spec(run_reqs: Any, target: str) -> str | None:
    """Rewrite the pixi-build-api-version entry in a requirements.run list.

    Preserves the surrounding list. Returns the previous spec when it was
    changed, otherwise None.
    """
    for i, entry in enumerate(run_reqs):
        spec = parse_api_requirement(str(entry))
        if spec is None:
            continue
        new_entry = f"{API_PKG} {target}".strip()
        if str(entry) != new_entry:
            run_reqs[i] = new_entry
            return spec
        return None
    return None


def set_recipe_specs(target: str) -> list[str]:
    """Set every backend run requirement in the all-backends recipe to target.

    Returns the names of the outputs that were changed.
    """
    yaml = YAML()
    yaml.preserve_quotes = True
    data: Any = yaml.load(ALL_BACKENDS_RECIPE)
    changed: list[str] = []
    for output in data.get("outputs", []):
        package = output.get("package")
        if not package or package.get("name") == API_PKG:
            continue
        run_reqs = output.get("requirements", {}).get("run")
        if not run_reqs:
            continue
        if rewrite_run_api_spec(run_reqs, target) is not None:
            changed.append(str(package.get("name")))
    if changed:
        yaml.dump(data, ALL_BACKENDS_RECIPE)
    return changed


def read_api_target(ref: str) -> str:
    """Read the agreed api-version requirement from the crate manifests at a ref."""
    specs = [
        str(
            tomlkit.parse(git_out("show", f"{ref}:{spec['pixi_manifest']}"))["package"][
                "run-dependencies"
            ][API_PKG]
        )
        for spec in BACKEND_DEFS
    ]
    return Counter(specs).most_common(1)[0][0]


def _cell(value: str | None, agreed: str) -> str:
    if value is None:
        return "[dim]-[/dim]"
    color = "green" if value == agreed else "red"
    return f"[{color}]{value}[/{color}]"


def show_api_matrix(
    crate_specs: dict[str, tuple[Path, str]],
    recipe_specs: dict[str, str],
    agreed: str,
) -> None:
    table = Table(title=f"{API_PKG} requirement")
    table.add_column("Backend", style="cyan")
    table.add_column("pixi.toml")
    table.add_column("all-backends")
    for binary in sorted(set(crate_specs) | set(recipe_specs)):
        pixi_spec = crate_specs.get(binary, (None, None))[1]
        table.add_row(
            binary,
            _cell(pixi_spec, agreed),
            _cell(recipe_specs.get(binary), agreed),
        )
    console.print(table)


def reconcile_api_version() -> None:
    """Show the api-version requirement across the local sites and reconcile them."""
    crate_specs = read_crate_specs()
    recipe_specs = discover_recipe_specs(YAML().load(ALL_BACKENDS_RECIPE))

    local_specs = [spec for _, spec in crate_specs.values()] + list(recipe_specs.values())
    agreed = Counter(local_specs).most_common(1)[0][0] if local_specs else ""

    show_api_matrix(crate_specs, recipe_specs, agreed)
    console.print()

    if len(set(local_specs)) <= 1:
        if not Confirm.ask(f"All sites agree on {agreed!r}. Bump it?", default=False):
            console.print("  [dim]Leaving api-version requirement unchanged.[/dim]")
            return
    else:
        console.print("  [yellow]Sites disagree; choose the value to reconcile them to.[/yellow]")

    while True:
        target = _ask(questionary.text(f"{API_PKG} requirement:", default=agreed)).strip()
        error = validate_spec(target)
        if error is None:
            break
        console.print(f"  [red]{error}[/red]")

    changed_files: list[str] = []
    for path, spec in crate_specs.values():
        if spec != target:
            set_crate_spec(path, target)
            changed_files.append(str(path.relative_to(ROOT)))
    changed_outputs = set_recipe_specs(target)

    if changed_files or changed_outputs:
        console.print(f"\n  Set {API_PKG} to [green]{target}[/green]:")
        for path_str in changed_files:
            console.print(f"    {path_str}")
        if changed_outputs:
            rel = ALL_BACKENDS_RECIPE.relative_to(ROOT)
            console.print(f"    {rel} ({', '.join(changed_outputs)})")
    else:
        console.print(f"\n  [dim]Local sites already at {target}.[/dim]")


# --- prompts ---


def _ask(question: Any) -> Any:
    """Ask a questionary question, exiting on Ctrl+C."""
    answer = question.ask()
    if answer is None:
        console.print("\n[dim]Interrupted.[/dim]")
        sys.exit(0)
    return answer


def select(message: str, choices: list[str], default: str | None = None) -> str:
    result: str = _ask(questionary.select(message, choices=choices, default=default))
    return result


# --- feedstocks ---


def existing_tags() -> set[str]:
    """Return the set of tag names that exist on the canonical remote."""
    tags: set[str] = set()
    for line in git_out("ls-remote", "--tags", REMOTE_URL).splitlines():
        _, _, ref = line.partition("\trefs/tags/")
        if ref:
            tags.add(ref.removesuffix("^{}"))
    return tags


def get_feedstock_version(clone_dir: Path) -> str:
    """Read the version from a feedstock's recipe.yaml."""
    data = YAML().load(clone_dir / "recipe" / "recipe.yaml")
    version: str = data["context"]["version"]
    return version


def compute_tarball_sha256(tag: str) -> str:
    """Download the source tarball for a tag and return its SHA-256 hash."""
    url = f"https://github.com/{REPO}/archive/refs/tags/{tag}.tar.gz"
    console.print(f"  Downloading [dim]{url}[/dim]")
    sha256 = hashlib.sha256()
    with urllib.request.urlopen(url) as response:  # noqa: S310
        while chunk := response.read(65536):
            sha256.update(chunk)
    digest = sha256.hexdigest()
    console.print(f"  SHA-256: [cyan]{digest}[/cyan]")
    return digest


def update_feedstock_recipe(
    recipe_path: Path, new_version: str, sha256: str, api_target: str
) -> None:
    """Update version, sha256, build number, and api-version in a feedstock recipe.

    The api-version requirement is only rewritten when it actually differs from
    api_target.
    """
    yaml = YAML()
    yaml.preserve_quotes = True
    data: Any = yaml.load(recipe_path)
    data["context"]["version"] = new_version
    data["source"]["sha256"] = sha256
    data["build"]["number"] = 0
    run_reqs = data.get("requirements", {}).get("run")
    if run_reqs is not None:
        old = rewrite_run_api_spec(run_reqs, api_target)
        if old is not None:
            console.print(f"  Set {API_PKG} {old} -> [green]{api_target}[/green]")
    yaml.dump(data, recipe_path)


def update_feedstock(backend: Backend, clone_dir: Path, api_target: str) -> None:
    """Update recipe, rerender, and open a PR for an already-cloned feedstock."""
    feedstock = backend.feedstock
    version = backend.new_version or backend.version
    branch = f"v{version}"

    console.print(f"\n  [bold]{feedstock}[/bold]")

    run(["git", "checkout", "-b", branch], cwd=clone_dir)

    sha256 = compute_tarball_sha256(backend.tag)

    recipe_path = clone_dir / "recipe" / "recipe.yaml"
    update_feedstock_recipe(recipe_path, version, sha256, api_target)
    console.print(f"  Updated {recipe_path.name}")

    run(["pixi", "exec", "conda-smithy", "rerender"], cwd=clone_dir)

    run(["git", "add", "--all"], cwd=clone_dir)
    run(["git", "commit", "--message", f"Update to {backend.binary} v{version}"], cwd=clone_dir)
    run(["git", "push", "--set-upstream", "--force", "origin", branch], cwd=clone_dir)

    run(
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


# --- subcommands ---


def bump() -> None:
    console.print("\n[bold]Backend Release - Step 1: bump[/bold]\n")

    if git_out("status", "--porcelain"):
        fail("working directory is not clean; commit or stash your changes first")

    console.print(f"Fetching main from {REPO}...")
    run(["git", "fetch", REMOTE_URL, "main"])
    run(["git", "switch", "-C", BUMP_BRANCH, "FETCH_HEAD"])

    backends = load_backends()
    show_versions(backends)
    console.print()

    updated: list[Backend] = []
    for b in backends:
        choice = select(
            f"{b.binary} ({b.version}):",
            ["skip", "patch", "minor", "major"],
            default="patch",
        )
        if choice != "skip":
            b.new_version = bump_version(b.version, choice)
            updated.append(b)
            console.print(f"  -> [bold green]{b.new_version}[/bold green]")
        else:
            console.print("  -> [dim]no change[/dim]")

    if updated:
        console.print("\n  Planned bumps:")
        for b in updated:
            console.print(f"    {b.binary}: {b.version} -> [green]{b.new_version}[/green]")
        for b in updated:
            set_version(b.version_path, b.new_version, b.version_table)
            console.print(f"  Updated {b.version_file}")

        workspace_updated = [b for b in updated if b.in_cargo_workspace]
        if workspace_updated:
            console.print()
            pkgs: list[str] = []
            for b in workspace_updated:
                pkgs.extend(["--package", b.binary])
            run(["cargo", "update", *pkgs])
    else:
        console.print("\n[yellow]No versions were bumped.[/yellow]")

    console.print(f"\n[bold]Reconcile {API_PKG}[/bold]\n")
    reconcile_api_version()

    if not git_out("status", "--porcelain"):
        console.print("\n[yellow]No changes to commit; nothing to do.[/yellow]")
        return

    console.print("\n[bold]Lint[/bold]\n")
    run(["pixi", "run", "--environment", "lefthook", "lint-fast"])

    console.print("\n[bold]Commit and open PR[/bold]\n")
    run(["git", "commit", "--all", "--message", "chore: bump backend versions"])

    # Force-push the disposable bump branch to the fork ourselves so a stale
    # remote branch from an earlier release attempt can never block the push
    # with a non-fast-forward rejection. gh then just opens the PR.
    login, target = fork_target()
    run(["git", "push", "--force", target, f"HEAD:refs/heads/{BUMP_BRANCH}"])
    run(
        [
            "gh",
            "pr",
            "create",
            "--repo",
            REPO,
            "--base",
            "main",
            "--head",
            f"{login}:{BUMP_BRANCH}",
            "--title",
            "chore: bump backend versions",
            "--body",
            "Automated backend version bump.",
        ]
    )

    sync_jj()
    console.print(
        "\n[bold green]Step 1 done![/bold green] "
        "Merge the PR, then run this script again and pick [cyan]Step 2: publish[/cyan]."
    )


def tag_backends(backends: list[Backend]) -> None:
    console.print("[bold]Tag[/bold]\n")
    tags_present = existing_tags()
    to_tag = [b for b in backends if b.tag not in tags_present]

    table = Table(title="Versions on main vs existing tags")
    table.add_column("Backend", style="cyan")
    table.add_column("Version", style="green")
    table.add_column("Status")
    for b in backends:
        status = "[green]will tag[/green]" if b in to_tag else "[dim]already tagged[/dim]"
        table.add_row(b.binary, b.version, status)
    console.print(table)

    if not to_tag:
        console.print(
            "\n  [yellow]No untagged backend versions on main. Has the bump PR merged yet?[/yellow]"
        )
        return

    console.print("\n  Tags to create:")
    for b in to_tag:
        console.print(f"    [cyan]{b.tag}[/cyan]")

    console.print()
    if not Confirm.ask("Proceed?", default=True):
        console.print("  [dim]Skipped tagging.[/dim]")
        return

    # Push tags straight from the fetched commit so no local tags are left behind
    # and a re-run after a failed push stays idempotent.
    refspecs = [f"FETCH_HEAD:refs/tags/{b.tag}" for b in to_tag]
    run(["git", "push", REMOTE_URL, *refspecs])
    for b in to_tag:
        console.print(f"  Pushed [cyan]{b.tag}[/cyan]")


def update_feedstocks(backends: list[Backend]) -> None:
    console.print("\n[bold]Feedstocks[/bold]\n")
    api_target = read_api_target("FETCH_HEAD")

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp = Path(tmpdir)
        outdated: list[tuple[Backend, Path]] = []

        for b in backends:
            clone_dir = tmp / b.feedstock.split("/")[1]
            run(["gh", "repo", "fork", b.feedstock, "--clone", "--default-branch-only"], cwd=tmp)
            # Sync the fork to upstream so our branch is based on the latest state.
            run(["gh", "repo", "sync", "--force"], cwd=clone_dir)
            run(["git", "pull", "--ff-only"], cwd=clone_dir)

            feedstock_version = get_feedstock_version(clone_dir)
            if feedstock_version == b.version:
                console.print(f"  [dim]{b.binary}: feedstock already at v{b.version}[/dim]")
                continue

            console.print(
                f"  [yellow]{b.binary}[/yellow]: feedstock v{feedstock_version} -> v{b.version}"
            )
            outdated.append((b, clone_dir))

        if not outdated:
            console.print("\n  [dim]All feedstocks are up to date.[/dim]")
            return

        console.print("\n  PRs to open:")
        for b, _ in outdated:
            console.print(f"    [cyan]{b.feedstock}[/cyan] -> v{b.version}")

        console.print()
        if not Confirm.ask("Proceed?", default=True):
            console.print("  [dim]Skipped feedstock updates.[/dim]")
            return

        console.print(f"\n  Reconciling {API_PKG} to [cyan]{api_target}[/cyan]\n")
        for b, clone_dir in outdated:
            update_feedstock(b, clone_dir, api_target)


def publish() -> None:
    console.print("\n[bold]Backend Release - Step 2: publish[/bold]\n")

    console.print(f"Fetching main from {REPO}...")
    run(["git", "fetch", REMOTE_URL, "main"])

    backends = load_backends(ref="FETCH_HEAD")
    show_versions(backends, title="Versions on main")
    console.print()

    tag_backends(backends)
    update_feedstocks(backends)

    sync_jj()
    console.print("\n[bold green]Done![/bold green]")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        nargs="?",
        choices=["bump", "publish"],
        help="skip the interactive prompt and run this step directly",
    )
    args = parser.parse_args()

    command = args.command or _ask(
        questionary.select(
            "Which step do you want to run?",
            choices=[
                questionary.Choice("Step 1: bump versions on a branch and open a PR", value="bump"),
                questionary.Choice(
                    "Step 2: after the PR merges, tag and update feedstocks",
                    value="publish",
                ),
            ],
        )
    )

    try:
        if command == "bump":
            bump()
        else:
            publish()
    except KeyboardInterrupt:
        console.print("\n[dim]Interrupted.[/dim]")


if __name__ == "__main__":
    main()
