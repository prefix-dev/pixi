# Use Ubuntu as base image for debugging (force x86_64/linux-64)
FROM ubuntu:22.04

# Install system dependencies
RUN apt-get update && apt-get install -y \
    curl \
    build-essential \
    pkg-config \
    libssl-dev \
    ca-certificates \
    git \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Set working directory
WORKDIR /workspace

# Copy the entire pixi repository
COPY . .

# Set working directory to the specific example
WORKDIR /workspace/examples/panic-panic

# Build pixi in debug mode for better debugging
RUN cargo build --bin pixi

# Run the failing command with maximum verbosity
CMD ["../../target/debug/pixi", "lock", "-vvv"]