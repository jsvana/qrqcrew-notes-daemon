# Build stage
FROM rust:slim-bookworm AS builder

WORKDIR /app

# Install build dependencies and nightly toolchain
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/* \
    && rustup toolchain install nightly \
    && rustup default nightly

# Copy source
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# Build release binary
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /app/target/release/qrqcrew-notes-daemon /usr/local/bin/

# Create non-root user
RUN useradd -r -s /bin/false qrqcrew
USER qrqcrew

ENTRYPOINT ["qrqcrew-notes-daemon"]
