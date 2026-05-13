import subprocess
import re
import os
import shutil
from pathlib import Path
import atexit
from enum import Enum

UPSTREAM_REPO = "prefix-dev/pixi"


class Colors(str, Enum):
    YELLOW = "\033[93m"
    MAGENTA = "\033[95m"
    RESET = "\033[0m"


status: list[str] = []


def colored_print(message: str, color: Colors) -> None:
    print(f"{color.value}{message}{Colors.RESET.value}")


def colored_input(prompt: str, color: Colors) -> str:
    return input(f"{color.value}{prompt}{Colors.RESET.value}")


def run_command(command: list[str | Path], capture_stdout: bool = False) -> str | None:
    colored_print(f"Running command: {' '.join([str(c) for c in command])}", Colors.YELLOW)
    result = subprocess.run(
        command, stdout=subprocess.PIPE if capture_stdout else None, stderr=None, text=True
    )
    if result.returncode != 0:
        colored_print(f"Error running command: {' '.join(map(str, command))}", Colors.YELLOW)
        exit(result.returncode)
    if capture_stdout:
        return result.stdout.strip()
    return None


def get_release_version() -> str:
    pattern = re.compile(r"^\d+\.\d+\.\d+$")
    version_from_env = os.environ.get("RELEASE_VERSION")

    if version_from_env and pattern.match(version_from_env):
        default_version = version_from_env
    else:
        default_version = ""

    while True:
        prompt = (
            f"Enter the release version (X.Y.Z) [{default_version}]: "
            if default_version
            else "Enter the release version (X.Y.Z): "
        )
        release_version = colored_input(prompt, Colors.MAGENTA) or default_version
        if pattern.match(release_version):
            return release_version
        else:
            colored_print(
                "Invalid format. Please enter the version in the format X.Y.Z where X, Y, and Z are integers.",
                Colors.YELLOW,
            )


def get_pixi() -> Path:
    pixi_bin = shutil.which("pixi")
    if pixi_bin:
        return Path(pixi_bin)
    else:
        raise ValueError("pixi not found in PATH")


def list_remotes() -> dict[str, str]:
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
    return remotes


def find_remote(remotes: dict[str, str], repo: str, preferred: list[str]) -> str | None:
    """Return the name of a remote whose URL contains `repo`.

    `preferred` is consulted first, in order, before falling back to any
    other matching remote.
    """
    for name in preferred:
        if name in remotes and repo in remotes[name]:
            return name
    for name, url in remotes.items():
        if repo in url:
            return name
    return None


def get_gh_user() -> str:
    """Return your GitHub username, from $GITHUB_USER or `gh api user`."""
    env_user = os.environ.get("GITHUB_USER", "").strip()
    if env_user:
        return env_user
    result = subprocess.run(
        ["gh", "api", "user", "--jq", ".login"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or not result.stdout.strip():
        colored_print(
            "Error: could not determine your GitHub username."
            + " Set $GITHUB_USER, or install and authenticate the gh CLI.",
            Colors.YELLOW,
        )
        exit(1)
    return result.stdout.strip()


def require_remote(remotes: dict[str, str], name: str, env_var: str) -> str:
    if name not in remotes:
        colored_print(
            f"Error: ${env_var} is set to '{name}', but no such git remote exists.",
            Colors.YELLOW,
        )
        exit(1)
    return name


def resolve_remotes() -> tuple[str, str]:
    """Return (upstream_remote, fork_remote): the remote names to pull
    from prefix-dev/pixi and to push the prep branch to your fork.

    Honors $UPSTREAM_REMOTE and $FORK_REMOTE as explicit overrides;
    otherwise resolves by matching remote URLs.
    """
    remotes = list_remotes()

    upstream_override = os.environ.get("UPSTREAM_REMOTE", "").strip()
    if upstream_override:
        upstream_remote = require_remote(remotes, upstream_override, "UPSTREAM_REMOTE")
    else:
        upstream_remote = find_remote(remotes, UPSTREAM_REPO, preferred=["upstream", "origin"])
        if upstream_remote is None:
            colored_print(
                f"Error: no git remote points to {UPSTREAM_REPO}."
                + " Set $UPSTREAM_REMOTE to override.",
                Colors.YELLOW,
            )
            exit(1)

    fork_override = os.environ.get("FORK_REMOTE", "").strip()
    if fork_override:
        fork_remote = require_remote(remotes, fork_override, "FORK_REMOTE")
    else:
        fork_repo = f"{get_gh_user()}/pixi"
        fork_remote = find_remote(remotes, fork_repo, preferred=["origin"])
        if fork_remote is None:
            colored_print(
                f"Error: no git remote points to your fork ({fork_repo})."
                + " Fork prefix-dev/pixi on GitHub and add it as a remote,"
                + " or set $FORK_REMOTE to override.",
                Colors.YELLOW,
            )
            exit(1)

    return upstream_remote, fork_remote


def print_summary() -> None:
    colored_print("\nSummary of completed steps:", Colors.YELLOW)
    for step in status:
        colored_print(f"- {step}", Colors.YELLOW)


atexit.register(print_summary)


def main() -> None:
    # Unset all PIXI_ prefixed environment variables to ensure a clean environment
    for key in list(os.environ.keys()):
        if key.startswith("PIXI_"):
            del os.environ[key]

    upstream_remote, fork_remote = resolve_remotes()
    colored_print(
        f"Using '{upstream_remote}' for {UPSTREAM_REPO} and '{fork_remote}' for your fork",
        Colors.YELLOW,
    )

    steps = [
        "Start release process",
        "Check main branch and CI status",
        "Set release version",
        "Create and switch to release branch",
        "Bump all versions",
        "Update cargo lockfile",
        "Update changelog",
        "Lint changes",
        "Commit changes",
        "Push changes",
        "Create and merge release prep PR",
    ]

    colored_print("Select the step to start from:", Colors.YELLOW)
    for i, step in enumerate(steps, 1):
        colored_print(f"{i}. {step}", Colors.YELLOW)

    while True:
        try:
            start_step = int(colored_input("Enter the step number: ", Colors.MAGENTA))
            if 1 <= start_step <= len(steps):
                break
            else:
                colored_print("Invalid step number. Please enter a valid number.", Colors.YELLOW)
        except ValueError:
            colored_print("Invalid input. Please enter a number.", Colors.YELLOW)

    pixi = get_pixi()

    step = 1
    try:
        if start_step <= 1:
            colored_print(f"{step}. Making a release of pixi", Colors.YELLOW)
            status.append("Started release process")
            step += 1

        if start_step <= 2:
            colored_input(
                f"{step}. Make sure main is up-to-date and CI passes. Press Enter to continue...",
                Colors.MAGENTA,
            )
            status.append("Checked main branch and CI status")
            step += 1

        release_version = get_release_version()
        os.environ["RELEASE_VERSION"] = release_version
        status.append(f"Release version set to {release_version}")
        step += 1

        if start_step <= 4:
            colored_print(f"\n{step}. Creating a new branch for the release...", Colors.YELLOW)
            run_command(["git", "checkout", "main"])
            run_command(["git", "pull", upstream_remote, "main"])
            branch = f"bump/prepare-v{release_version}"

            branch_exists = run_command(["git", "branch", "--list", branch], capture_stdout=True)
            if branch_exists:
                run_command(["git", "branch", "--delete", branch])
            run_command(["git", "switch", "--create", branch])
            status.append(f"Created and switched to branch {branch}")
            step += 1

        if start_step <= 5:
            colored_print(f"\n{step}. Bumping all versions...", Colors.YELLOW)
            run_command([pixi, "run", "bump"])
            status.append("Bumped all versions")
            step += 1

        if start_step <= 6:
            colored_print(f"\n{step}. Update Cargo pixi lockfile...", Colors.YELLOW)
            run_command([pixi, "run", "cargo update pixi"])
            status.append("Updated all lockfile")
            step += 1

        if start_step <= 7:
            while True:
                response = (
                    colored_input(
                        f"{step}. Should we bump the changelog? (yes/no): ", Colors.MAGENTA
                    )
                    .strip()
                    .lower()
                )
                if response.lower() in ["yes", "no", "y", "n"]:
                    break
                else:
                    colored_print("Invalid response. Please enter 'yes' or 'no'.", Colors.YELLOW)
            if response == "yes" or response == "y":
                run_command([pixi, "run", "bump-changelog"])
            colored_input(
                "Don't forget to update the 'Highlights' section in `CHANGELOG.md`. Press Enter to continue...",
                Colors.MAGENTA,
            )
            status.append("Updated the changelog")
            step += 1

        if start_step <= 8:
            colored_print(f"\n{step}. Linting the changes...", Colors.YELLOW)
            run_command([pixi, "run", "lint"])
            step += 1

        if start_step <= 9:
            colored_print(f"\n{step}. Committing the changes...", Colors.YELLOW)
            run_command(["git", "commit", "-am", f"chore: version to {release_version}"])
            status.append("Committed the changes")
            step += 1

        if start_step <= 10:
            colored_print(f"\n{step}. Pushing the changes...", Colors.YELLOW)
            run_command(
                [
                    "git",
                    "push",
                    "--set-upstream",
                    fork_remote,
                    f"bump/prepare-v{release_version}",
                ]
            )
            status.append("Pushed the changes")
            step += 1

        if start_step <= 11:
            colored_print(f"\n{step}. Release prep PR", Colors.YELLOW)
            colored_input(
                "Create a PR to check off the change with the peers. Press Enter to continue...",
                Colors.MAGENTA,
            )
            colored_input("Merge that PR. Press Enter to continue...", Colors.MAGENTA)
            status.append("Created and merged the release prep PR")
            step += 1

        colored_print(
            f"\nStart a release build for 'v{release_version}' by starting the workflow manually in https://github.com/prefix-dev/pixi/actions/workflows/release.yml",
            Colors.YELLOW,
        )

        colored_print("\nDONE!", Colors.YELLOW)
        status.append("Release process completed successfully")

    except KeyboardInterrupt:
        colored_print("\nProcess interrupted.", Colors.YELLOW)


if __name__ == "__main__":
    main()
