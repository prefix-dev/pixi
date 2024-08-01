import subprocess

PIXI_VERSION = "0.26.1"


def verify_cli_command(
    command: str,
    expected_exit_code: int | None = None,
    stdout_contains: str | None = None,
    stderr_contains: str | None = None,
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
    if stdout_contains is not None:
        assert stdout_contains in stdout.strip(), f"Unexpected stdout: {stdout.strip()}"
    if stderr_contains is not None:
        assert stderr_contains in stderr.strip(), f"Unexpected stderr: {stderr.strip()}"


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
