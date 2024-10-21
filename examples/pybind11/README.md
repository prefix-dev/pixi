# pybind11

This example demonstrates how to compile pybind11 modules using Pixi and `scikit-build-core`.

## Overview

[pybind11](https://github.com/pybind/pybind11) is a lightweight header-only library that exposes C++ types in Python and vice versa, mainly to create Python bindings of existing C++ code. This project showcases an efficient way to build pybind11 modules using modern tools.

## Prerequisites

- [Pixi](https://github.com/prefix-dev/pixi) installed on your system
- Basic knowledge of C++ and Python

## Usage

To build the project, run the following command in your terminal:

```sh
pixi run build
```

This command will:

1. Compile the C++ code
2. Generate Python bindings
3. Create a `dist` folder containing:
   - A source distribution (`.tar.gz`) file
   - A wheel (`.whl`) file

The source distribution includes the wheel file along with other project-related files, making it ready for publication to a Python package repository like PyPI.

## Installing the wheel

After building the project, you can install the generated wheel file for local testing. Follow these steps:

1. Navigate to the `dist` folder:
   ```sh
   cd dist
   ```

2. Install the wheel file using pip:
   ```sh
   pip install your_package_name-version-py3-none-any.whl
   ```
   Replace `your_package_name-version-py3-none-any.whl` with the actual name of your wheel file.

3. You can now import and use your module in Python:
   ```python
   import your_module_name
   # Use your module here
   ```

## Next Steps

- Explore the generated files in the `dist` folder
- Test the compiled module in a Python environment
- Consider publishing your package to PyPI or a private repository
- Create a different environment per Python version to generate multiple `.whl` files.

For more information on pybind11, visit the [official documentation](https://pybind11.readthedocs.io/).
