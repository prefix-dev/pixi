from setuptools import setup


def get_version():
    return "0.6.0"


setup(
    name="setup_project",
    version=get_version(),
    description="A package with setup.py for dynamic metadata testing",
    packages=["setup_project"],
)
