#!/bin/bash
set -e

echo "=========================================="
echo "Testing with locally built pixi (no git)"
echo "=========================================="
echo ""

# Check if pixi binary exists
if [ ! -f "../../target/release/pixi" ]; then
    echo "Building pixi in release mode..."
    cd ../..
    cargo build --release
    cd examples/panic-panic
fi

echo "Building Docker image with local pixi binary..."

# Create a Dockerfile that uses the local binary
cat >Dockerfile.local <<'EOF'
FROM ubuntu:22.04

# Install only curl, NOT git - this is intentional
RUN apt-get update && \
    apt-get install -y curl ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the locally built pixi binary
COPY ../../target/release/pixi /usr/local/bin/pixi
RUN chmod +x /usr/local/bin/pixi

# Copy the test configuration
COPY pixi.toml.test pixi.toml

# Try to install - this should fail with a clear error about git being missing
CMD ["sh", "-c", "pixi install 2>&1 || true"]
EOF

docker build -f Dockerfile.local -t pixi-local-no-git-test .

echo ""
echo "=========================================="
echo "Running pixi install with LOCAL build"
echo "=========================================="
echo ""

docker run --rm pixi-local-no-git-test

echo ""
echo "=========================================="
echo "Test complete!"
echo "=========================================="
echo ""
echo "Check if error message shows full chain:"
echo "  ✓ 'Git executable not found' should be visible"
echo "  ✓ Full error chain should be displayed"
echo "  ✗ Should NOT only show 'Failed to do lookahead resolution'"

# Cleanup
rm -f Dockerfile.local
