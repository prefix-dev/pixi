#!/usr/bin/env -S pixi exec --with mkcert==1.4.4 --with pip==25.3 --with python==3.14.1 --with rich==14.2.0 --with testcontainers==4.13.3 python
"""
Test script for the tls-root-certs feature.

Use the local build pixi with:
--pixi-bin $CARGO_TARGET_DIR/release/pixi

Requires sudo access to install mkcert CA into system trust store.

Sets up a local HTTPS PyPI server with a custom CA (via mkcert) to verify that:
- tls-root-certs=webpki: Fails (mkcert CA not in webpki-roots)
- tls-root-certs=native: Succeeds (mkcert CA in system trust store)

Requirements:
- Docker running
- mkcert installed
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from rich.console import Console
from rich.panel import Panel
from rich.markup import escape
from testcontainers.core.container import DockerContainer

console = Console()

NGINX_CONFIG = """\
events {
    worker_connections 1024;
}

http {
    server {
        listen 443 ssl;
        server_name localhost;

        ssl_certificate /etc/nginx/cert.pem;
        ssl_certificate_key /etc/nginx/key.pem;

        location / {
            root /usr/share/nginx/html;
            autoindex on;
            autoindex_format html;
        }

        location /simple/ {
            alias /usr/share/nginx/html/simple/;
            autoindex on;
            autoindex_format html;
            default_type text/html;
        }
    }
}
"""

SIMPLE_INDEX_HTML = """\
<!DOCTYPE html>
<html>
  <head><title>Simple Index</title></head>
  <body>
    <a href="test-pkg/">test-pkg</a>
  </body>
</html>
"""

PACKAGE_INDEX_HTML = """\
<!DOCTYPE html>
<html>
  <head><title>Links for test-pkg</title></head>
  <body>
    <h1>Links for test-pkg</h1>
    <a href="/packages/{wheel_name}">{wheel_name}</a>
  </body>
</html>
"""

PIXI_TOML_TEMPLATE = """\
[workspace]
name = "native-certs-test"
version = "0.1.0"
channels = ["conda-forge"]
platforms = ["osx-arm64", "osx-64", "linux-64"]

[dependencies]
python = ">=3.11"

[pypi-options]
extra-index-urls = ["https://localhost:{port}/simple/"]

[pypi-dependencies]
test-pkg = "*"
"""


def check_prerequisites() -> bool:
    """Check that required tools are available."""
    missing = []

    if not shutil.which("mkcert"):
        missing.append("mkcert")

    if not shutil.which("docker"):
        missing.append("docker")

    # Check if Docker is running
    if shutil.which("docker"):
        result = subprocess.run(["docker", "info"], check=False, capture_output=True, text=True)
        if result.returncode != 0:
            console.print("[red]Docker is installed but not running[/red]")
            return False

    if missing:
        console.print(f"[red]Missing required tools: {', '.join(missing)}[/red]")
        return False

    return True


def setup_mkcert() -> None:
    """Install mkcert CA into system trust store."""
    console.print("[yellow]Setting up mkcert CA...[/yellow]")
    subprocess.run(["mkcert", "-install"], check=True)
    console.print("[green]✓ mkcert CA installed in system trust store[/green]")


def generate_certs(test_dir: Path) -> tuple[Path, Path]:
    """Generate TLS certificates for localhost."""
    console.print("[yellow]Generating certificates...[/yellow]")
    cert_file = test_dir / "cert.pem"
    key_file = test_dir / "key.pem"
    subprocess.run(
        [
            "mkcert",
            "-cert-file",
            str(cert_file),
            "-key-file",
            str(key_file),
            "localhost",
            "127.0.0.1",
        ],
        check=True,
    )
    console.print("[green]✓ Certificates generated[/green]")
    return cert_file, key_file


def create_test_package(test_dir: Path) -> Path:
    """Create a minimal Python wheel package."""
    console.print("[yellow]Creating test package...[/yellow]")
    packages_dir = test_dir / "packages"
    pkg_dir = packages_dir / "test-pkg"
    pkg_dir.mkdir(parents=True)

    (pkg_dir / "setup.py").write_text(
        "from setuptools import setup\n"
        "setup(name='test-pkg', version='1.0.0', py_modules=['test_pkg'])\n"
    )
    (pkg_dir / "test_pkg.py").write_text("# Test package\n")

    subprocess.run(
        [sys.executable, "-m", "pip", "wheel", ".", "-w", str(packages_dir)],
        cwd=pkg_dir,
        capture_output=True,
        check=True,
    )
    console.print("[green]✓ Test package created[/green]")
    return packages_dir


def create_simple_index(test_dir: Path, packages_dir: Path) -> Path:
    """Create PEP 503 simple index."""
    console.print("[yellow]Creating PyPI simple index...[/yellow]")
    simple_dir = test_dir / "simple"
    pkg_index_dir = simple_dir / "test-pkg"
    pkg_index_dir.mkdir(parents=True)

    (simple_dir / "index.html").write_text(SIMPLE_INDEX_HTML)

    wheel_files = list(packages_dir.glob("*.whl"))
    if not wheel_files:
        raise RuntimeError("No wheel file found")
    wheel_name = wheel_files[0].name

    (pkg_index_dir / "index.html").write_text(PACKAGE_INDEX_HTML.format(wheel_name=wheel_name))
    console.print("[green]✓ Simple index created[/green]")
    return simple_dir


def create_nginx_config(test_dir: Path) -> Path:
    """Create nginx configuration for HTTPS."""
    config_file = test_dir / "nginx.conf"
    config_file.write_text(NGINX_CONFIG)
    return config_file


def run_pixi_test(pixi_bin: str, project_dir: Path, tls_root_certs: str) -> tuple[bool, str]:
    """Run pixi install with specified tls-root-certs setting."""
    # Clean up lock file
    lock_file = project_dir / "pixi.lock"
    if lock_file.exists():
        lock_file.unlink()

    env = os.environ.copy()
    env["PIXI_TLS_ROOT_CERTS"] = tls_root_certs

    result = subprocess.run(
        [pixi_bin, "install"],
        cwd=project_dir,
        env=env,
        capture_output=True,
        text=True,
    )
    output = result.stdout + result.stderr
    return result.returncode == 0, output


def main() -> int:
    parser = argparse.ArgumentParser(description="Test tls-root-certs feature")
    parser.add_argument(
        "--pixi-bin",
        default=os.environ.get("PIXI_BIN"),
        help="Path to pixi binary (default: $PIXI_BIN or 'pixi')",
    )
    parser.add_argument(
        "--keep",
        action="store_true",
        help="Keep test directory after completion",
    )
    args = parser.parse_args()

    # Resolve pixi-bin path relative to current working directory before we cd elsewhere
    if not args.pixi_bin:
        pixi_bin = "pixi"
    else:
        pixi_bin = Path(args.pixi_bin)
        if not pixi_bin.is_absolute():
            pixi_bin = Path.cwd() / pixi_bin
        pixi_bin = str(pixi_bin.resolve())

    console.print(Panel("[bold]TLS Root Certs Feature Test[/bold]", style="yellow"))

    if not check_prerequisites():
        return 1

    test_dir = Path(tempfile.mkdtemp(prefix="pixi-native-certs-"))
    console.print(f"Test directory: {test_dir}")

    try:
        # Setup
        setup_mkcert()
        cert_file, key_file = generate_certs(test_dir)
        packages_dir = create_test_package(test_dir)
        simple_dir = create_simple_index(test_dir, packages_dir)
        nginx_conf = create_nginx_config(test_dir)

        # Start nginx container with testcontainers
        console.print("[yellow]Starting HTTPS PyPI server...[/yellow]")

        with (
            DockerContainer("nginx:alpine")
            .with_exposed_ports(443)
            .with_volume_mapping(str(nginx_conf), "/etc/nginx/nginx.conf", "ro")
            .with_volume_mapping(str(cert_file), "/etc/nginx/cert.pem", "ro")
            .with_volume_mapping(str(key_file), "/etc/nginx/key.pem", "ro")
            .with_volume_mapping(str(simple_dir), "/usr/share/nginx/html/simple", "ro")
            .with_volume_mapping(str(packages_dir), "/usr/share/nginx/html/packages", "ro")
        ) as nginx:
            # Wait for nginx to be ready (check logs for startup message)
            import time

            time.sleep(3)  # Give nginx time to start
            port = nginx.get_exposed_port(443)
            console.print(f"[green]✓ PyPI server running at https://localhost:{port}[/green]")

            # Create test project
            console.print("[yellow]Creating test pixi project...[/yellow]")
            project_dir = test_dir / "pixi-test-project"
            project_dir.mkdir()
            (project_dir / "pixi.toml").write_text(PIXI_TOML_TEMPLATE.format(port=port))
            console.print("[green]✓ Test project created[/green]")

            # Run tests
            console.print(Panel("[bold]Running Tests[/bold]", style="yellow"))

            # Test A: With webpki certs (should fail)
            console.print("\n[yellow]Test A: With tls-root-certs=webpki (should FAIL)[/yellow]")
            console.print("The CA is NOT in webpki-roots, so this should fail\n")
            success_a, output_a = run_pixi_test(pixi_bin, project_dir, tls_root_certs="webpki")
            cert_error = any(term in output_a.lower() for term in ["certificate", "ssl", "tls"])
            test_a_passed = not success_a or cert_error
            if test_a_passed:
                console.print("[green]✓ Test A PASSED: Got expected certificate error[/green]")
            else:
                console.print("[red]✗ Test A FAILED: Unexpected success[/red]")

            # Test B: With native certs (should succeed)
            console.print("\n[yellow]Test B: With tls-root-certs=native (should SUCCEED)[/yellow]")
            console.print("The CA IS in the system trust store, so this should work\n")
            success_b, output_b = run_pixi_test(pixi_bin, project_dir, tls_root_certs="native")
            test_b_passed = success_b
            if test_b_passed:
                console.print("[green]✓ Test B PASSED: Install succeeded with native certs[/green]")
            else:
                console.print("[red]✗ Test B FAILED: Install failed[/red]")
                console.print(f"Output: {escape(output_b)}")

        # Summary
        console.print(Panel("[bold]Test Summary[/bold]", style="yellow"))

        if test_a_passed and test_b_passed:
            console.print("[green]All tests PASSED![/green]")
            console.print("\nThe tls-root-certs feature is working correctly:")
            console.print("  - tls-root-certs=webpki: Uses webpki-roots (mkcert CA not trusted)")
            console.print("  - tls-root-certs=native: Uses system store (mkcert CA trusted)")
            return 0
        else:
            console.print("[red]Some tests FAILED[/red]")
            console.print(f"Test A (should fail with tls-root-certs=webpki): {test_a_passed}")
            console.print(f"Test B (should pass with tls-root-certs=native): {test_b_passed}")
            return 1

    except Exception as e:
        console.print(f"[red]Error: {escape(str(e))}[/red]")
        return 1

    finally:
        if not args.keep:
            shutil.rmtree(test_dir, ignore_errors=True)
            console.print("Cleaned up test directory")
        else:
            console.print(f"Keeping test directory: {test_dir}")


if __name__ == "__main__":
    sys.exit(main())
