"""Generate attestation signature files for conda packages."""

import argparse
import shutil
import tempfile
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate attestation signature files for conda packages"
    )
    parser.add_argument(
        "attestation_bundle",
        type=Path,
        help="Path to the attestation bundle file",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(tempfile.gettempdir()) / "rattler-build-output",
        help="Directory containing conda packages (default: %(default)s)",
    )
    args = parser.parse_args()

    if not args.attestation_bundle.exists():
        parser.error(f"Attestation bundle not found: {args.attestation_bundle}")

    # Find all conda packages recursively
    conda_packages = list(args.output_dir.rglob("*.conda"))

    if not conda_packages:
        parser.error(f"No conda packages found in {args.output_dir}")

    # Copy attestation bundle for each package
    for conda_package in conda_packages:
        sig_name = conda_package.stem + ".sig"
        sig_path = conda_package.parent / sig_name
        shutil.copy(args.attestation_bundle, sig_path)
        print(f"Created attestation: {sig_path}")


if __name__ == "__main__":
    main()
