import subprocess
import re
import os
from pathlib import Path


def run_command(command: list[str], capture_stdout=False) -> str | None:
    print(f"Running command: {' '.join([str(c) for c in command])}")
    result = subprocess.run(
        command, stdout=subprocess.PIPE if capture_stdout else None, stderr=None, text=True
    )
    if result.returncode != 0:
        print(f"Error running command: {' '.join(command)}")
        exit(result.returncode)
    if capture_stdout:
        return result.stdout.strip()
    return None


def get_release_version():
    pattern = re.compile(r"^\d+\.\d+\.\d+$")
    while True:
        release_version = input("Enter the release version (X.Y.Z): ")
        if pattern.match(release_version):
            return release_version
        else:
            print(
                "Invalid format. Please enter the version in the format X.Y.Z where X, Y, and Z are integers."
            )


def get_pixi() -> Path:
    pixi_bin = Path().home().joinpath(".pixi/bin/pixi").resolve()

    if pixi_bin.is_file() and pixi_bin.exists():
        return pixi_bin
    else:
        raise ValueError(f"The path {pixi_bin} doesn't exist.")


def main():
    print("Making a release of pixi")
    pixi = get_pixi()

    # Prep
    input("Make sure main is up-to-date and CI passes. Press Enter to continue...")

    release_version = get_release_version()
    os.environ["RELEASE_VERSION"] = release_version

    print("\nCreating a new branch for the release...")
    run_command(["git", "checkout", "main"])
    run_command(["git", "pull", "upstream", "main"])
    branch = f"bump/prepare-v{release_version}"

    branch_exists = run_command(["git", "branch", "--list", branch], capture_stdout=True)
    if branch_exists:
        run_command(["git", "branch", "--delete", branch])
    run_command(["git", "switch", "--create", branch])

    print("\nBumping all versions...")
    run_command([pixi, "run", "bump"])

    print("\nUpdating the changelog...")
    run_command([pixi, "run", "bump-changelog"])
    input(
        "Don't forget to update the 'Highlights' section in `CHANGELOG.md`. Press Enter to continue..."
    )

    print("\nCommitting the changes...")
    run_command(["git", "commit", "-am", f"chore: version to {release_version}"])

    print("\nPushing the changes...")
    run_command(["git", "push", "origin"])

    # Release prep PR
    print("\nRelease prep PR")
    input("Create a PR to check off the change with the peers. Press Enter to continue...")
    input("Merge that PR. Press Enter to continue...")

    # Tagging the release
    print("\nTagging the release")
    print("\nChecking out main...")
    run_command(["git", "fetch", "upstream"])
    run_command(["git", "checkout", "upstream/main"])

    print("\nTagging the release...")
    run_command(["git", "tag", f"v{release_version}", "-m", f"Release {release_version}"])

    print("\nPushing the tag...")
    run_command(["git", "push", "upstream", f"v{release_version}"])

    # Publishing the release
    input(
        "Update the Release which has CI created for you (after the first build) and add the changelog to the release notes. Press Enter to continue..."
    )
    input("Make sure all the artifacts are there and the CI is green!!! Press Enter to continue...")
    input("Publish the release and make sure it is set as latest. Press Enter to continue...")

    # Test the release using the install script
    print("Testing the release using `pixi self-update`...")
    run_command([pixi, "self-update"])

    version_output = run_command([pixi, "--version"], capture_stdout=True)
    expected_version_output = f"pixi {release_version}"
    if version_output == expected_version_output:
        print(f"Version check passed: {version_output}")
    else:
        print(f"Version check failed: expected {expected_version_output}, got {version_output}")

        print("\nDONE!")


if __name__ == "__main__":
    main()
