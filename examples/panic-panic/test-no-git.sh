#!/bin/bash
set -e

echo "=========================================="
echo "Testing pixi error handling without git"
echo "=========================================="
echo ""

# Build the test docker image
echo "Building Docker image without git..."
docker build -f Dockerfile.test -t pixi-no-git-test .

echo ""
echo "=========================================="
echo "Running pixi install (should fail gracefully)"
echo "=========================================="
echo ""

# Run the container and capture output
docker run --rm pixi-no-git-test

echo ""
echo "=========================================="
echo "Test complete!"
echo "=========================================="
echo ""
echo "Expected: Clear error message about git being missing"
echo "Not expected: Unclear Tokio panic message"
