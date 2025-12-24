#!/usr/bin/env python3

from setuptools import setup, find_packages

# print("About to raise an exception during setup...")
# raise RuntimeError("This is an intentional panic during setup.py execution!")

setup(
    name="panic-panic",
    version="0.1.0",
    description="A package that raises exceptions",
    packages=find_packages(),
    python_requires=">=3.8",
    install_requires=[],
)
