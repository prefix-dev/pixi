# Output Dependencies Test Workspace

This workspace contains a simple test package used to test the `get_output_dependencies` function.

## Structure

- `test-package/`: A package with build, host, and run dependencies
  - Build dependencies: cmake, make
  - Host dependencies: zlib, openssl
  - Run dependencies: python, numpy
