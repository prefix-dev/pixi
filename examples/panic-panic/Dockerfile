FROM ubuntu:22.04

# Install build dependencies but NOT git - this is intentional to reproduce the issue
RUN apt-get update && \
    apt-get install -y curl build-essential && \
    rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:$PATH"

# Copy the pixi source code from repository root
# Note: Build this with: docker build -f examples/panic-panic/Dockerfile .
COPY . /pixi-source
WORKDIR /pixi-source

# Build pixi from source
RUN cargo build

# Add pixi to PATH
ENV PATH="/pixi-source/target/debug:$PATH"

WORKDIR /app

# Copy the test configuration
COPY examples/panic-panic/pixi.toml pixi.toml

# Try to install - this should fail with a clear error about git being missing
# We don't use RUN here so the container build doesn't fail
CMD ["sh", "-c", "pixi install 2>&1 || true"]
