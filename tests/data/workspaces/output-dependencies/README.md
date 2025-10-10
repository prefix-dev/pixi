# Output Dependencies Test Workspace

This workspace is used to test the `get_output_dependencies` and `expand_dev_sources` functionality of the command dispatcher.

## Structure

### test-package
A basic package with various dependency types:
- **Build dependencies**: cmake, make
- **Host dependencies**: openssl, zlib
- **Run dependencies**: numpy, python

### package-a
A package that depends on other packages to test dev source filtering:
- **Build dependencies**: gcc
- **Host dependencies**: test-package (path dependency)
- **Run dependencies**: package-b (path dependency), requests

### package-b
A simple package used to test non-dev-source dependencies:
- **Run dependencies**: curl

## Test Scenarios

### test_expand_dev_sources
Tests the dev sources expansion functionality with:
1. **Simple case**: test-package as a dev source
2. **Recursive filtering**: package-a depends on test-package (both are dev sources)
   - test-package should be **filtered out** from dependencies (it's a dev source)
3. **Non-dev-source inclusion**: package-a depends on package-b
   - package-b should be **included** in dependencies (it's not a dev source)
