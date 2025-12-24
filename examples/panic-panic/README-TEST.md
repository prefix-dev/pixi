# Testing Git Error Handling

This directory contains test files to reproduce and verify the fix for issue #4831.

## Issue Summary

When a PyPI package requires git (e.g., `chumpy = { git = "..." }`) but git is not installed,
pixi should show a clear error message indicating that git is missing, not just a generic
Tokio panic message.

## Test Files

- `pixi.toml.test` - Test configuration with a git dependency
- `Dockerfile.test` - Docker image WITHOUT git (uses official pixi release)
- `test-no-git.sh` - Script to run the test with official pixi
- `test-local-no-git.sh` - Script to run the test with locally built pixi
- `Dockerfile.local` - Created dynamically by test-local-no-git.sh

## Running Tests

### Option 1: Test with Official Pixi Release

```bash
cd examples/panic-panic
./test-no-git.sh
```

This will:
1. Build a Docker image without git
2. Install pixi from official source
3. Try to install dependencies (will fail)
4. Show the error message

### Option 2: Test with Local Build (Recommended)

```bash
cd examples/panic-panic
./test-local-no-git.sh
```

This will:
1. Build pixi in release mode (if not already built)
2. Create a Docker image without git
3. Copy your locally built pixi into the container
4. Try to install dependencies (will fail)
5. Show the error message with your fix applied

## Expected Output

### Before Fix (Bad)
```
Error: × failed to solve the pypi requirements of environment 'default' for platform 'linux-64'
╰─▶ unexpected panic during PyPI resolution: Failed to do lookahead resolution: Failed to download and build `chumpy @ git+https://github.com/mattloper/chumpy`
```

### After Fix (Good)
```
Error: × failed to solve the pypi requirements of environment 'default' for platform 'linux-64'
╰─▶ unexpected panic during PyPI resolution: Failed to do lookahead resolution: Failed to download and build `chumpy @ git+https://github.com/mattloper/chumpy`
    Git operation failed
    Git executable not found. Ensure that Git is installed and available.
```

The key difference is that the **full error chain** is now visible, including the root cause:
"Git executable not found."

## Cleanup

```bash
# Remove test docker images
docker rmi pixi-no-git-test
docker rmi pixi-local-no-git-test
```
