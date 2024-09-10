import sys


def pytest_addoption(parser):
    if sys.platform.startswith("win"):
        parser.addoption(
            "--pixi-exec", action="store", default="pixi.exe", help="Path to the pixi executable"
        )
    else:
        parser.addoption(
            "--pixi-exec", action="store", default="pixi", help="Path to the pixi executable"
        )
