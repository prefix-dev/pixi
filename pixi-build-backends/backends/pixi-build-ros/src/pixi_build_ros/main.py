from .ros_generator import ROSGenerator
from pixi_build_backend.main import run_backend


def main() -> None:
    """Main entry point for the script."""
    generator = ROSGenerator()
    run_backend(generator)
