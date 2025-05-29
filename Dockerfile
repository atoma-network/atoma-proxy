# Builder stage
FROM --platform=$BUILDPLATFORM ubuntu:24.04 AS builder

# Add platform-specific arguments
ARG TARGETPLATFORM
ARG BUILDPLATFORM
ARG TARGETARCH

ARG PROFILE

# Install build dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install Rust 1.87.0
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.87.0 \
    && . "$HOME/.cargo/env"

# Add cargo to PATH
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /usr/src/atoma-proxy

COPY . .


# Compile
RUN if [ "$PROFILE" = "cloud" ]; then \
    cargo build --release --bin atoma-proxy --features google-oauth; \
    else \
    cargo build --release --bin atoma-proxy; \
    fi

# Final stage
FROM ubuntu:24.04

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Create necessary directories
RUN mkdir -p /app/logs

# Copy the built binary from builder stage
COPY --from=builder /usr/src/atoma-proxy/target/release/atoma-proxy /usr/local/bin/atoma-proxy

# Set executable permissions explicitly
RUN chmod +x /usr/local/bin/atoma-proxy

# Copy and set up entrypoint script
COPY entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/entrypoint.sh

# Copy host client.yaml and modify keystore path
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]

CMD ["/usr/local/bin/atoma-proxy", "--config-path", "/app/config.toml"]
