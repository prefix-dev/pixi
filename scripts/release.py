"""Cut a pixi release.

Branches from prefix-dev/pixi@main, bumps the version, updates the changelog
and lock file, commits, and opens a PR.

Because it always branches from the canonical remote's main, it behaves the
same in a plain git clone or a colocated jj repo, regardless of which branch
(or detached HEAD) you happen to be on.

Usage:
    pixi run release

Shows the commits since the last release and asks for a major / minor / patch
bump.
"""

import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

import questionary

ROOT = Path(__file__).resolve().parent.parent
REPO = "prefix-dev/pixi"
REMOTE_URL = f"https://github.com/{REPO}.git"
CHANGELOG = ROOT / "CHANGELOG.md"
CARGO_TOML = "crates/pixi/Cargo.toml"

Version = tuple[int, int, int]


def run(cmd: list[str], *, cwd: Path = ROOT) -> None:
    print(f"  → {' '.join(cmd)}")
    subprocess.run(cmd, check=True, cwd=cwd, text=True)


def git_out(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, text=True, capture_output=True).stdout.strip()


def fail(msg: str) -> None:
    print(f"\nerror: {msg}", file=sys.stderr)
    sys.exit(1)


def is_jj() -> bool:
    """Whether ROOT is a colocated jj repo with the jj binary available."""
    return (ROOT / ".jj").is_dir() and shutil.which("jj") is not None


def sync_jj() -> None:
    """Import the git refs created by this script into a colocated jj repo."""
    if not is_jj():
        return
    print("Importing git refs into jj...")
    run(["jj", "git", "import"])


def parse(version: str) -> Version:
    parts = version.split(".")
    if len(parts) != 3 or not all(p.isdigit() for p in parts):
        fail(f"cannot parse version '{version}' as X.Y.Z")
    major, minor, patch = (int(p) for p in parts)
    return major, minor, patch


def fmt(version: Version) -> str:
    return ".".join(str(n) for n in version)


def fetched_version() -> Version:
    """Version in crates/pixi/Cargo.toml on the freshly fetched canonical main."""
    cargo = git_out("show", f"FETCH_HEAD:{CARGO_TOML}")
    return parse(tomllib.loads(cargo)["package"]["version"])


def gh_token() -> str:
    """A GitHub token from gh CLI auth, used to enrich git-cliff output."""
    return subprocess.run(
        ["gh", "auth", "token"], cwd=ROOT, text=True, capture_output=True
    ).stdout.strip()


def cliff_preview(tag: str) -> str:
    """Render what git-cliff would add for the commits since `tag`."""
    return subprocess.run(
        [
            "git-cliff",
            "--strip",
            "header",
            "--github-token",
            gh_token(),
            f"{tag}..FETCH_HEAD",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
    ).stdout.strip()


def bump_changelog(version: str) -> None:
    """Prepend the unreleased changes to CHANGELOG.md under a new version tag."""
    run(
        [
            "git-cliff",
            "--unreleased",
            "--prepend",
            str(CHANGELOG),
            "--github-token",
            gh_token(),
            "--tag",
            f"v{version}",
        ]
    )


def latest_tag() -> str:
    tag = git_out("describe", "--tags", "--abbrev=0", "--match", "v*", "FETCH_HEAD")
    if not tag:
        fail("no v* tag reachable from canonical main")
    return tag


def select_version(current: Version) -> str:
    major, minor, patch = current
    options: dict[str, Version] = {
        "major": (major + 1, 0, 0),
        "minor": (major, minor + 1, 0),
        "patch": (major, minor, patch + 1),
    }
    choices = [
        questionary.Choice(f"{kind:<5} → {fmt(version)}", value=fmt(version))
        for kind, version in options.items()
    ]
    answer = questionary.select("Select the bump:", choices=choices, default=choices[-1]).ask()
    if answer is None:
        fail("aborted")
    return answer


def changelog_section(version: str) -> str:
    """Extract the section for `version` from CHANGELOG.md for the PR body."""
    content = CHANGELOG.read_text()
    pattern = rf"(### \[{re.escape(version)}\].*?)(?=\n### \[|\n---|\Z)"
    match = re.search(pattern, content, re.DOTALL)
    return match.group(1).strip() if match else f"Release {version}"


def main() -> None:
    if git_out("status", "--porcelain"):
        if is_jj():
            # In a colocated jj repo, in-progress work lives committed in @ and
            # shows as dirty to git. Set it aside with `jj new` so git sees a
            # clean tree for the upcoming `git switch`; @- stays as a recoverable
            # loose head.
            print("Working copy is dirty; running `jj new` to set it aside...")
            run(["jj", "new"])
        else:
            fail("working directory is not clean; commit or stash first")

    print(f"Fetching canonical main from {REPO}...")
    run(["git", "fetch", REMOTE_URL, "main"])

    current = fetched_version()
    tag = latest_tag()
    if parse(tag.lstrip("v")) != current:
        fail(
            f"{CARGO_TOML} on main ({fmt(current)}) is ahead of the latest tag "
            f"({tag}); a release may already be pending"
        )

    print(f"\nChangelog preview since {tag}:")
    print(cliff_preview(tag) or "  (none)")

    version = select_version(current)

    print(f"\n=== Releasing {version} ===\n")

    branch = f"bump/prepare-v{version}"
    run(["git", "switch", "-C", branch, "FETCH_HEAD"])

    print("Patching version files...")
    run(["tbump", "--non-interactive", "--only-patch", version])

    print("Updating changelog...")
    bump_changelog(version)
    input("Edit the '✨ Highlights' section in CHANGELOG.md, then press Enter...")

    print("Updating Cargo.lock...")
    run(["cargo", "update", "pixi"])

    print("Committing...")
    run(["git", "commit", "--all", "--message", f"chore: bump version to v{version}"])

    print("Opening pull request...")
    run(
        [
            "gh",
            "pr",
            "create",
            "--repo",
            REPO,
            "--base",
            "main",
            "--title",
            f"chore: bump version to v{version}",
            "--body",
            changelog_section(version),
        ]
    )

    sync_jj()

    print("\n=== Done ===")
    print(f"Opened release PR for {version}.")
    print(
        "After merge, run the Release workflow via workflow_dispatch:\n"
        "https://github.com/prefix-dev/pixi/actions/workflows/release.yml"
    )


if __name__ == "__main__":
    main()
