from .python_generator import PythonGenerator
from pixi_build_backend.main import run_backend


def main() -> None:
    """Main entry point for the script."""
    generator = PythonGenerator()
    run_backend(generator)
