"""
Main entry point for Python backend implementation.
"""

import sys
from pixi_build_backend.types.generated_recipe import GenerateRecipeProtocol
from pixi_build_backend.pixi_build_backend import py_main, py_main_sync, PyGenerateRecipe


async def run_backend_async(instance: GenerateRecipeProtocol) -> None:
    """Async version of the main entry point for the build backend"""
    py_generator = PyGenerateRecipe(instance)

    try:
        await py_main(py_generator, sys.argv)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


def run_backend(instance: GenerateRecipeProtocol) -> None:
    """Sync version of the main entry point for the build backend"""
    py_generator = PyGenerateRecipe(instance)

    try:
        py_main_sync(py_generator, sys.argv)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)
