import subprocess
import re
import os
from pathlib import Path
import atexit

COLORS = {"yellow": "\033[93m", "magenta": "\033[95m", "reset": "\033[0m"}

status = []


def colored_print(message: str, color: str):
    color_code = COLORS.get(color, None)
    if color_code:
        print(f"{color_code}{message}{COLORS['reset']}")
    else:
        print(message)


def colored_input(prompt: str, color: str) -> str:
    color_code = COLORS.get(color, COLORS["reset"])
    return input(f"{color_code}{prompt}{COLORS['reset']}")


def run_command(command: list[str], capture_stdout=False) -> str | None:
    colored_print(f"Running command: {' '.join([str(c) for c in command])}", "yellow")
    result = subprocess.run(
        command, stdout=subprocess.PIPE if capture_stdout else None, stderr=None, text=True
    )
    if result.returncode != 0:
        colored_print(f"Error running command: {' '.join(map(str, command))}", "yellow")
        exit(result.returncode)
    if capture_stdout:
        return result.stdout.strip()
    return None


def get_release_version():
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
        release_version = colored_input(prompt, "magenta") or default_version
        if pattern.match(release_version):
            return release_version
        else:
            colored_print(
                "Invalid format. Please enter the version in the format X.Y.Z where X, Y, and Z are integers.",
                "yellow",
            )


def get_pixi() -> Path:
    pixi_bin = Path().home().joinpath(".pixi/bin/pixi").resolve()

    if pixi_bin.is_file() and pixi_bin.exists():
        return pixi_bin
    else:
        raise ValueError(f"The path {pixi_bin} doesn't exist.")


def print_summary():
    colored_print("\nSummary of completed steps:", "yellow")
    for step in status:
        colored_print(f"- {step}", "yellow")


atexit.register(print_summary)


def main():
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
        "Tag release",
        "Push tag",
        "Publish release",
        "Test release",
    ]

    colored_print("Select the step to start from:", "yellow")
    for i, step in enumerate(steps, 1):
        colored_print(f"{i}. {step}", "yellow")

    while True:
        try:
            start_step = int(colored_input("Enter the step number: ", "magenta"))
            if 1 <= start_step <= len(steps):
                break
            else:
                colored_print("Invalid step number. Please enter a valid number.", "yellow")
        except ValueError:
            colored_print("Invalid input. Please enter a number.", "yellow")

    pixi = get_pixi()
    try:
        if start_step <= 1:
            colored_print("Making a release of pixi", "yellow")
            status.append("Started release process")

        if start_step <= 2:
            colored_input(
                "Make sure main is up-to-date and CI passes. Press Enter to continue...", "magenta"
            )
            status.append("Checked main branch and CI status")

        release_version = get_release_version()
        os.environ["RELEASE_VERSION"] = release_version
        status.append(f"Release version set to {release_version}")

        if start_step <= 4:
            colored_print("\nCreating a new branch for the release...", "yellow")
            run_command(["git", "checkout", "main"])
            run_command(["git", "pull", "upstream", "main"])
            branch = f"bump/prepare-v{release_version}"

            branch_exists = run_command(["git", "branch", "--list", branch], capture_stdout=True)
            if branch_exists:
                run_command(["git", "branch", "--delete", branch])
            run_command(["git", "switch", "--create", branch])
            status.append(f"Created and switched to branch {branch}")

        if start_step <= 5:
            colored_print("\nBumping all versions...", "yellow")
            run_command([pixi, "run", "bump"])
            status.append("Bumped all versions")

        if start_step <= 6:
            while True:
                response = (
                    colored_input("Should we bump the changelog? (yes/no): ", "magenta")
                    .strip()
                    .lower()
                )
                if response.lower() in ["yes", "no", "y", "n"]:
                    break
                else:
                    colored_print("Invalid response. Please enter 'yes' or 'no'.", "yellow")
            if response == "yes" or response == "y":
                run_command([pixi, "run", "bump-changelog"])
            colored_input(
                "Don't forget to update the 'Highlights' section in `CHANGELOG.md`. Press Enter to continue...",
                "magenta",
            )
            status.append("Updated the changelog")

        if start_step <= 7:
            colored_print("\nLinting the changes...", "yellow")
            run_command([pixi, "run", "lint"])

        if start_step <= 8:
            colored_print("\nCommitting the changes...", "yellow")
            run_command(["git", "commit", "-am", f"chore: version to {release_version}"])
            status.append("Committed the changes")

        if start_step <= 9:
            colored_print("\nPushing the changes...", "yellow")
            run_command(["git", "push", "origin"])
            status.append("Pushed the changes")

        if start_step <= 10:
            colored_print("\nRelease prep PR", "yellow")
            colored_input(
                "Create a PR to check off the change with the peers. Press Enter to continue...",
                "magenta",
            )
            colored_input("Merge that PR. Press Enter to continue...", "magenta")
            status.append("Created and merged the release prep PR")

        if start_step <= 11:
            colored_print("\nTagging the release", "yellow")
            colored_print("\nChecking out main...", "yellow")
            run_command(["git", "fetch", "upstream"])
            run_command(["git", "checkout", "upstream/main"])

            colored_print("\nTagging the release...", "yellow")
            run_command(["git", "tag", f"v{release_version}", "-m", f"Release {release_version}"])
            status.append(f"Tagged the release with version {release_version}")

        if start_step <= 12:
            colored_print("\nPushing the tag...", "yellow")
            run_command(["git", "push", "upstream", f"v{release_version}"])
            status.append("Pushed the tag")

        if start_step <= 13:
            colored_input(
                "Update the Release which has CI created for you (after the first build) and add the changelog to the release notes. Press Enter to continue...",
                "magenta",
            )
            colored_input(
                "Make sure all the artifacts are there and the CI is green!!! Press Enter to continue...",
                "magenta",
            )
            colored_input(
                "Publish the release and make sure it is set as latest. Press Enter to continue...",
                "magenta",
            )
            status.append("Published the release")

            colored_print("Testing the release using `pixi self-update`...", "yellow")
            run_command([pixi, "self-update"])

            version_output = run_command([pixi, "--version"], capture_stdout=True)
            expected_version_output = f"pixi {release_version}"
            if version_output == expected_version_output:
                colored_print(f"Version check passed: {version_output}", "yellow")
            else:
                colored_print(
                    f"Version check failed: expected {expected_version_output}, got {version_output}",
                    "yellow",
                )
            status.append("Tested the release")

            colored_print("\nDONE!", "yellow")
            status.append("Release process completed successfully")

    except KeyboardInterrupt:
        colored_print("\nProcess interrupted.", "yellow")


if __name__ == "__main__":
    main()
