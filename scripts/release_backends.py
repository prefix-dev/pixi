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

UPSTREAM_REPO = "prefix-dev/pixi"
USE_JJ = Path(".jj").is_dir()

# Name of the protocol package whose requirement range is kept identical across
# every backend declaration site.
API_PKG = "pixi-build-api-version"

# The all-backends recipe declares per-output run requirements and is also the
# read-only source of truth for context.api_version (the version of the
# pixi-build-api-version package itself).
ALL_BACKENDS_RECIPE = Path("pixi-build-backends/recipe/all-backends/recipe.yaml")

# Each entry needs "binary", "version_file", "pixi_manifest", and "feedstock".
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

STEPS = [
    "Choose version bumps",
    "Apply version bumps and update lock files",
    "Reconcile api-version requirement",
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
        return self.binary

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
    doc = tomlkit.parse(path.read_text())
    version = doc[table]["version"]
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
    doc = tomlkit.parse(path.read_text())
    doc[table]["version"] = new_version
    path.write_text(tomlkit.dumps(doc))


def parse_api_requirement(entry: str) -> str | None:
    """Extract the spec from a "pixi-build-api-version <spec>" run requirement.

    Returns the spec portion (e.g. ">=5,<6"), an empty string when the package is
    listed without a spec, or None when the entry is for another package.
    """
    spec = MatchSpec(entry)
    if str(spec.name.normalized) != API_PKG:
        return None
    return str(spec.version) if spec.version is not None else ""


def read_crate_specs() -> dict[str, tuple[Path, str]]:
    """Map backend binary name to its crate pixi.toml path and api-version spec."""
    specs: dict[str, tuple[Path, str]] = {}
    for spec in BACKEND_DEFS:
        path = Path(spec["pixi_manifest"])
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


def gh_fetch_file(repo: str, path: str, ref: str | None = None) -> str | None:
    """Fetch a file's raw content from GitHub, returning None on failure."""
    endpoint = f"repos/{repo}/contents/{path}"
    if ref:
        endpoint += f"?ref={ref}"
    result = subprocess.run(
        ["gh", "api", "-H", "Accept: application/vnd.github.raw", endpoint],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    return result.stdout


def fetch_feedstock_spec(feedstock: str) -> str | None:
    """Read the api-version spec from a feedstock recipe on its default branch.

    Raises RuntimeError when the recipe cannot be fetched. Returns None when the
    recipe carries no pixi-build-api-version run requirement.
    """
    content = gh_fetch_file(feedstock, "recipe/recipe.yaml")
    if content is None:
        raise RuntimeError(f"could not fetch recipe for {feedstock}")
    data: Any = YAML().load(content)
    run_reqs = data.get("requirements", {}).get("run") or []
    for entry in run_reqs:
        spec = parse_api_requirement(str(entry))
        if spec is not None:
            return spec
    return None


def validate_spec(spec: str) -> str | None:
    """Return an error message when spec does not parse as a version spec."""
    try:
        VersionSpec(spec)
    except InvalidVersionSpecError as exc:
        return f"not a valid version spec: {exc}"
    return None


def read_target_from_main() -> str:
    """Read the agreed api-version requirement from the local sites on main.

    Exits when a site cannot be read or when main carries internal drift.
    """
    specs: dict[str, str] = {}
    for spec in BACKEND_DEFS:
        path = spec["pixi_manifest"]
        content = gh_fetch_file(UPSTREAM_REPO, path, ref="main")
        if content is None:
            console.print(f"[bold red]Could not read {path} from main[/bold red]")
            sys.exit(1)
        doc: Any = tomlkit.parse(content)
        specs[path] = str(doc["package"]["run-dependencies"][API_PKG])

    content = gh_fetch_file(UPSTREAM_REPO, str(ALL_BACKENDS_RECIPE), ref="main")
    if content is None:
        console.print(f"[bold red]Could not read {ALL_BACKENDS_RECIPE} from main[/bold red]")
        sys.exit(1)
    data = YAML().load(content)
    for name, spec in discover_recipe_specs(data).items():
        specs[f"recipe:{name}"] = spec

    unique = set(specs.values())
    if len(unique) != 1:
        console.print("[bold red]main has internal api-version drift:[/bold red]")
        for site, spec in specs.items():
            console.print(f"    {site}: {spec}")
        sys.exit(1)
    return unique.pop()


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
    result: str = _ask(questionary.select(message, choices=choices, default=default))
    return result


def checkbox(message: str, backends: list[Backend]) -> list[Backend]:
    choices: list[Any] = [
        questionary.Choice(f"{b.binary} v{b.version}", value=i, checked=True)
        for i, b in enumerate(backends)
    ]
    selected_indices: list[int] = _ask(questionary.checkbox(message, choices=choices))
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
    yaml = YAML()
    data = yaml.load(clone_dir / "recipe" / "recipe.yaml")
    version: str = data["context"]["version"]
    return version


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


def update_feedstock_recipe(
    recipe_path: Path, new_version: str, sha256: str, api_target: str | None = None
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
    if api_target is not None:
        run_reqs = data.get("requirements", {}).get("run")
        if run_reqs is not None:
            old = rewrite_run_api_spec(run_reqs, api_target)
            if old is not None:
                console.print(f"  Set {API_PKG} {old} -> [green]{api_target}[/green]")
    yaml.dump(data, recipe_path)


def run_in_dir(cmd: list[str], cwd: Path) -> None:
    """Run a command in a specific directory."""
    console.print(f"[dim]$ {' '.join(cmd)}  (in {cwd})[/dim]")
    result = subprocess.run(cmd, cwd=cwd)
    if result.returncode != 0:
        console.print(f"[bold red]Command failed:[/bold red] {' '.join(cmd)}")
        sys.exit(1)


def update_feedstock(backend: Backend, clone_dir: Path, api_target: str | None = None) -> None:
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
    update_feedstock_recipe(recipe_path, version, sha256, api_target)
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


@dataclass
class ApiRow:
    binary: str
    pixi_spec: str | None
    recipe_spec: str | None
    feedstock_spec: str | None
    feedstock_error: bool = False


def build_api_rows(
    crate_specs: dict[str, tuple[Path, str]],
    recipe_specs: dict[str, str],
    feedstock_by_binary: dict[str, str],
) -> list[ApiRow]:
    rows: list[ApiRow] = []
    for binary in sorted(set(crate_specs) | set(recipe_specs)):
        pixi_spec = crate_specs.get(binary, (None, None))[1]
        recipe_spec = recipe_specs.get(binary)
        feedstock_spec: str | None = None
        feedstock_error = False
        try:
            feedstock_spec = fetch_feedstock_spec(feedstock_by_binary[binary])
        except RuntimeError as exc:
            feedstock_error = True
            console.print(f"  [yellow]warning:[/yellow] {exc}")
        rows.append(ApiRow(binary, pixi_spec, recipe_spec, feedstock_spec, feedstock_error))
    return rows


def _cell(value: str | None, agreed: str) -> str:
    if value is None:
        return "[dim]-[/dim]"
    color = "green" if value == agreed else "red"
    return f"[{color}]{value}[/{color}]"


def _feedstock_cell(row: ApiRow, agreed: str) -> str:
    if row.feedstock_error:
        return "[yellow]?[/yellow]"
    return _cell(row.feedstock_spec, agreed)


def show_api_matrix(rows: list[ApiRow], agreed: str) -> None:
    table = Table(title=f"{API_PKG} requirement")
    table.add_column("Backend", style="cyan")
    table.add_column("pixi.toml")
    table.add_column("all-backends")
    table.add_column("conda-forge")
    for row in rows:
        table.add_row(
            row.binary,
            _cell(row.pixi_spec, agreed),
            _cell(row.recipe_spec, agreed),
            _feedstock_cell(row, agreed),
        )
    console.print(table)


def reconcile_api_version() -> None:
    """Show the api-version requirement across all sites and reconcile the local ones."""
    crate_specs = read_crate_specs()
    recipe_specs = discover_recipe_specs(YAML().load(ALL_BACKENDS_RECIPE))
    feedstock_by_binary = {d["binary"]: d["feedstock"] for d in BACKEND_DEFS}

    rows = build_api_rows(crate_specs, recipe_specs, feedstock_by_binary)

    local_specs = [s for row in rows for s in (row.pixi_spec, row.recipe_spec) if s is not None]
    comparable = local_specs + [
        row.feedstock_spec for row in rows if row.feedstock_spec is not None
    ]
    agreed = Counter(comparable).most_common(1)[0][0] if comparable else ""

    show_api_matrix(rows, agreed)
    console.print()

    current = Counter(local_specs).most_common(1)[0][0] if local_specs else agreed

    if len(set(comparable)) <= 1:
        if not Confirm.ask(f"All sites agree on {current!r}. Bump it?", default=False):
            console.print("  [dim]Leaving api-version requirement unchanged.[/dim]")
            return
    else:
        console.print(
            "  [yellow]Sites disagree; choose the value to reconcile the local sites to.[/yellow]"
        )

    while True:
        target = _ask(questionary.text(f"{API_PKG} requirement:", default=current)).strip()
        error = validate_spec(target)
        if error is None:
            break
        console.print(f"  [red]{error}[/red]")

    changed_files: list[str] = []
    for _binary, (path, spec) in crate_specs.items():
        if spec != target:
            set_crate_spec(path, target)
            changed_files.append(str(path))
    changed_outputs = set_recipe_specs(target)

    if changed_files or changed_outputs:
        console.print(f"\n  Set {API_PKG} to [green]{target}[/green]:")
        for path_str in changed_files:
            console.print(f"    {path_str}")
        if changed_outputs:
            console.print(f"    {ALL_BACKENDS_RECIPE} ({', '.join(changed_outputs)})")
    else:
        console.print(f"\n  [dim]Local sites already at {target}.[/dim]")


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

        # Step 2: Apply version bumps and update lock files
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

            completed.append("Applied version bumps and updated lock files")

        # Step 3: Reconcile api-version requirement
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")
            reconcile_api_version()
            completed.append("Reconciled api-version requirement")

        # Step 4: Run linting
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")
            run(["pixi", "run", "--environment", "lefthook", "lint-fast"])
            completed.append("Linting passed")

        # Step 5: Commit and push changes
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")
            branch = "bump/backends-release"
            commit_and_push(remote, branch, "chore: bump backend versions")
            completed.append(f"Committed and pushed to {branch}")

        # Step 6: Create and merge PR
        step += 1
        if start_step <= step:
            console.print(f"\n[bold]Step {step}. {STEPS[step - 1]}[/bold]\n")
            console.print("  Create a PR with the version bump changes and get it merged.")
            Confirm.ask("PR created and merged?", default=False)
            completed.append("PR created and merged")

        # Step 7: Choose backends to tag
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

        # Step 8: Create tags and push
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

        # Step 9: Update conda-forge feedstocks
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
                        # Read the target from main so this step does not depend on
                        # local working-copy state when resumed directly.
                        api_target = read_target_from_main()
                        console.print(
                            f"  Reconciling {API_PKG} to [cyan]{api_target}[/cyan] on main\n"
                        )
                        for b, clone_dir in outdated:
                            update_feedstock(b, clone_dir, api_target)
                        completed.append("Updated conda-forge feedstocks")
                    else:
                        console.print("[dim]Skipped feedstock updates.[/dim]")

        console.print("\n[bold green]Done![/bold green]")

    except KeyboardInterrupt:
        console.print("\n[dim]Interrupted.[/dim]")


if __name__ == "__main__":
    main()
