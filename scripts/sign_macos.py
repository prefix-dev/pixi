"""Codesign and notarize a macOS binary in place.

Apple's notarization service does not accept tar.gz archives, so the signed
binary is submitted inside a temporary zip. Notarization does not modify the
binary itself - once the submission succeeds, the signed binary on disk is ready
to be packaged as tar.gz by the subsequent packaging step.

Expects the following environment variables:
    CODESIGN_CERTIFICATE          - Base64-encoded .p12 certificate
    CODESIGN_CERTIFICATE_PASSWORD - Certificate password
    CODESIGN_IDENTITY             - Signing identity
    APPLEID_USERNAME              - Apple ID for notarization
    APPLEID_PASSWORD              - App-specific password
    APPLEID_TEAMID                - Apple Developer Team ID

Usage:
    pixi run -e release sign-macos --binary target/aarch64-apple-darwin/release/pixi
"""

import argparse
import base64
import os
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

KEYCHAIN_NAME = "release-signing.keychain-db"
KEYCHAIN_PASSWORD = "release-signing-password"


def run(cmd: list[str], *, capture_output: bool = False) -> subprocess.CompletedProcess[str]:
    print(f"  -> {' '.join(cmd)}")
    return subprocess.run(cmd, check=True, text=True, capture_output=capture_output)


def setup_keychain(cert_path: Path, cert_password: str) -> None:
    run(["security", "create-keychain", "-p", KEYCHAIN_PASSWORD, KEYCHAIN_NAME])
    run(["security", "set-keychain-settings", "-lut", "21600", KEYCHAIN_NAME])
    run(["security", "unlock-keychain", "-p", KEYCHAIN_PASSWORD, KEYCHAIN_NAME])
    run(
        [
            "security",
            "import",
            str(cert_path),
            "-k",
            KEYCHAIN_NAME,
            "-P",
            cert_password,
            "-T",
            "/usr/bin/codesign",
        ]
    )
    run(
        [
            "security",
            "set-key-partition-list",
            "-S",
            "apple-tool:,apple:,codesign:",
            "-s",
            "-k",
            KEYCHAIN_PASSWORD,
            KEYCHAIN_NAME,
        ]
    )
    result = run(["security", "list-keychains", "-d", "user"], capture_output=True)
    existing = [line.strip().strip('"') for line in result.stdout.splitlines() if line.strip()]
    run(["security", "list-keychains", "-d", "user", "-s", KEYCHAIN_NAME, *existing])


def codesign(binary: Path, identity: str) -> None:
    print(f"\nSigning {binary}...")
    run(["codesign", "--force", "--options", "runtime", "--sign", identity, str(binary)])


def notarize(binary: Path, username: str, password: str, team_id: str) -> None:
    print(f"\nNotarizing {binary}...")
    with tempfile.TemporaryDirectory() as tmpdir:
        archive = Path(tmpdir) / f"{binary.name}.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zf:
            zf.write(binary, arcname=binary.name)
        run(
            [
                "xcrun",
                "notarytool",
                "submit",
                str(archive),
                "--apple-id",
                username,
                "--password",
                password,
                "--team-id",
                team_id,
                "--wait",
            ]
        )


def main() -> None:
    parser = argparse.ArgumentParser(description="Codesign and notarize a macOS binary")
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()

    binary: Path = args.binary
    if not binary.is_file():
        print(f"error: {binary} is not a file", file=sys.stderr)
        sys.exit(1)

    cert_b64 = os.environ["CODESIGN_CERTIFICATE"]
    cert_password = os.environ["CODESIGN_CERTIFICATE_PASSWORD"]
    identity = os.environ["CODESIGN_IDENTITY"]
    apple_username = os.environ["APPLEID_USERNAME"]
    apple_password = os.environ["APPLEID_PASSWORD"]
    apple_team_id = os.environ["APPLEID_TEAMID"]

    with tempfile.NamedTemporaryFile(suffix=".p12", delete=False) as f:
        f.write(base64.b64decode(cert_b64))
        cert_path = Path(f.name)

    try:
        print("Setting up signing keychain...")
        setup_keychain(cert_path, cert_password)
        codesign(binary, identity)
        notarize(binary, apple_username, apple_password, apple_team_id)
    finally:
        cert_path.unlink(missing_ok=True)

    print(f"\nSigned and notarized {binary}")


if __name__ == "__main__":
    main()
