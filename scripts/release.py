import subprocess
import re
import os
from pathlib import Path
import atexit
from enum import Enum


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
    pixi_bin = Path().home().joinpath(".pixi/bin/pixi").resolve()

    if pixi_bin.is_file() and pixi_bin.exists():
        return pixi_bin
    else:
        raise ValueError(f"The path {pixi_bin} doesn't exist.")


def print_summary() -> None:
    colored_print("\nSummary of completed steps:", Colors.YELLOW)
    for step in status:
        colored_print(f"- {step}", Colors.YELLOW)


atexit.register(print_summary)


def main() -> None:
    steps = [
        "Start release process",
        "Check main branch and CI status",
        "Set release version",
        "Create and switch to release branch",
        "Bump all versions",
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
    try:
        if start_step <= 1:
            colored_print("Making a release of pixi", Colors.YELLOW)
            status.append("Started release process")

        if start_step <= 2:
            colored_input(
                "Make sure main is up-to-date and CI passes. Press Enter to continue...",
                Colors.MAGENTA,
            )
            status.append("Checked main branch and CI status")

        release_version = get_release_version()
        os.environ["RELEASE_VERSION"] = release_version
        status.append(f"Release version set to {release_version}")

        if start_step <= 4:
            colored_print("\nCreating a new branch for the release...", Colors.YELLOW)
            run_command(["git", "checkout", "main"])
            run_command(["git", "pull", "upstream", "main"])
            branch = f"bump/prepare-v{release_version}"

            branch_exists = run_command(["git", "branch", "--list", branch], capture_stdout=True)
            if branch_exists:
                run_command(["git", "branch", "--delete", branch])
            run_command(["git", "switch", "--create", branch])
            status.append(f"Created and switched to branch {branch}")

        if start_step <= 5:
            colored_print("\nBumping all versions...", Colors.YELLOW)
            run_command([pixi, "run", "bump"])
            status.append("Bumped all versions")

        if start_step <= 6:
            while True:
                response = (
                    colored_input("Should we bump the changelog? (yes/no): ", Colors.MAGENTA)
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

        if start_step <= 7:
            colored_print("\nLinting the changes...", Colors.YELLOW)
            run_command([pixi, "run", "lint"])

        if start_step <= 8:
            colored_print("\nCommitting the changes...", Colors.YELLOW)
            run_command(["git", "commit", "-am", f"chore: version to {release_version}"])
            status.append("Committed the changes")

        if start_step <= 9:
            colored_print("\nPushing the changes...", Colors.YELLOW)
            run_command(["git", "push", "origin"])
            status.append("Pushed the changes")

        if start_step <= 10:
            colored_print("\nRelease prep PR", Colors.YELLOW)
            colored_input(
                "Create a PR to check off the change with the peers. Press Enter to continue...",
                Colors.MAGENTA,
            )
            colored_input("Merge that PR. Press Enter to continue...", Colors.MAGENTA)
            status.append("Created and merged the release prep PR")

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
