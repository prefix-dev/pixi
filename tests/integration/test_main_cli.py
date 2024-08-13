import subprocess

PIXI_VERSION = "0.27.1"


def verify_cli_command(
    command: str,
    expected_exit_code: int | None = None,
    stdout_contains: str | list | None = None,
    stdout_excludes: str | list | None = None,
    stderr_contains: str | list | None = None,
    stderr_excludes: str | list | None = None,
):
    process = subprocess.Popen(
        command, shell=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    stdout, stderr = process.communicate()
    print(f"command: {command}, stdout: {stdout}, stderr: {stderr}, code: {process.returncode}")

    if expected_exit_code is not None:
        assert int(process.returncode) == int(
            expected_exit_code
        ), f"Return code was {process.returncode}, stderr: {stderr}"

    if expected_exit_code is not None:
        assert (
            process.returncode == expected_exit_code
        ), f"Return code was {process.returncode}, expected {expected_exit_code}, stderr: {stderr}"

    if stdout_contains:
        if isinstance(stdout_contains, str):
            stdout_contains = [stdout_contains]
        for substring in stdout_contains:
            assert substring in stdout, f"'{substring}' not found in stdout: {stdout}"

    if stdout_excludes:
        if isinstance(stdout_excludes, str):
            stdout_excludes = [stdout_excludes]
        for substring in stdout_excludes:
            assert substring not in stdout, f"'{substring}' unexpectedly found in stdout: {stdout}"

    if stderr_contains:
        if isinstance(stderr_contains, str):
            stderr_contains = [stderr_contains]
        for substring in stderr_contains:
            assert substring in stderr, f"'{substring}' not found in stderr: {stderr}"

    if stderr_excludes:
        if isinstance(stderr_excludes, str):
            stderr_excludes = [stderr_excludes]
        for substring in stderr_excludes:
            assert substring not in stderr, f"'{substring}' unexpectedly found in stderr: {stderr}"


def test_pixi():
    verify_cli_command("pixi", 2, None, f"[version {PIXI_VERSION}]")
    verify_cli_command("pixi --version", 0, PIXI_VERSION, None)


def test_project_commands(tmp_path):
    manifest_path = tmp_path / "pixi.toml"
    # Create a new project
    verify_cli_command(f"pixi init {tmp_path}", 0)

    # Channel commands
    verify_cli_command(f"pixi project --manifest-path {manifest_path} channel add bioconda", 0)
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} channel list", 0, stdout_contains="bioconda"
    )
    verify_cli_command(f"pixi project --manifest-path {manifest_path} channel remove bioconda", 0)

    # Description commands
    verify_cli_command(f"pixi project --manifest-path {manifest_path} description set blabla", 0)
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} description get", 0, stdout_contains="blabla"
    )

    # Environment commands
    verify_cli_command(f"pixi project --manifest-path {manifest_path} environment add test", 0)
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} environment list", 0, stdout_contains="test"
    )
    verify_cli_command(f"pixi project --manifest-path {manifest_path} environment remove test", 0)
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} environment list", 0, stdout_excludes="test"
    )

    # Platform commands
    verify_cli_command(f"pixi project --manifest-path {manifest_path} platform add linux-64", 0)
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} platform list", 0, stdout_contains="linux-64"
    )
    verify_cli_command(f"pixi project --manifest-path {manifest_path} platform remove linux-64", 0)
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} platform list", 0, stdout_excludes="linux-64"
    )

    # Version commands
    verify_cli_command(f"pixi project --manifest-path {manifest_path} version set 1.2.3", 0)
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} version get", 0, stdout_contains="1.2.3"
    )
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} version major", 0, stderr_contains="2.2.3"
    )
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} version minor", 0, stderr_contains="2.3.3"
    )
    verify_cli_command(
        f"pixi project --manifest-path {manifest_path} version patch", 0, stderr_contains="2.3.4"
    )


def test_global_install():
    # Install
    verify_cli_command("pixi global install rattler-build", 0, None, "rattler-build")

    # TODO: fix this, not working because of the repodata gateway
    # verify_cli_command('pixi global install rattler-build -c https://fast.prefix.dev/conda-forge', 0, None, "rattler-build")

    # Upgrade
    verify_cli_command("pixi global upgrade rattler-build", 0)

    # List
    verify_cli_command("pixi global list", 0, stderr_contains="rattler-build")

    # Remove
    verify_cli_command("pixi global remove rattler-build", 0)
    verify_cli_command("pixi global remove rattler-build", 1)


def test_search():
    verify_cli_command(
        "pixi search rattler-build -c conda-forge", 0, stdout_contains="rattler-build"
    )
    # TODO: fix this, not working because of the repodata gateway
    # verify_cli_command('pixi search rattler-build -c https://fast.prefix.dev/conda-forge', 0, stdout_contains="rattler-build")


def test_simple_project_setup(tmp_path):
    manifest_path = tmp_path / "pixi.toml"
    # Create a new project
    verify_cli_command(f"pixi init {tmp_path}", 0)

    # Add package
    verify_cli_command(
        f"pixi add --manifest-path {manifest_path}  _r-mutex", 0, stderr_contains="Added"
    )
    verify_cli_command(
        f"pixi add --manifest-path {manifest_path} --feature test _r-mutex==1.0.1",
        0,
        stderr_contains=["test", "==1.0.1"],
    )
    verify_cli_command(
        f"pixi add --manifest-path {manifest_path} --platform linux-64 conda-forge::_r-mutex",
        0,
        stderr_contains=["linux-64", "conda-forge"],
    )
    verify_cli_command(
        f"pixi add --manifest-path {manifest_path} -f test -p osx-arm64 _r-mutex",
        0,
        stderr_contains=["osx-arm64", "test"],
    )

    # Remove package
    verify_cli_command(
        f"pixi remove --manifest-path {manifest_path} _r-mutex", 0, stderr_contains="Removed"
    )
    verify_cli_command(
        f"pixi remove --manifest-path {manifest_path} --feature test _r-mutex",
        0,
        stderr_contains=["test", "Removed"],
    )
    verify_cli_command(
        f"pixi remove --manifest-path {manifest_path} --platform linux-64 conda-forge::_r-mutex",
        0,
        stderr_contains=["linux-64", "conda-forge", "Removed"],
    )
    verify_cli_command(
        f"pixi remove --manifest-path {manifest_path} -f test -p osx-arm64 _r-mutex",
        0,
        stderr_contains=["osx-arm64", "test", "Removed"],
    )
