# Build stage
FROM rust:1.87-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy Cargo files first for better caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
RUN mkdir -p src/grpc && \
    echo "fn main() {}" > src/main.rs && \
    echo "fn main() {}" > src/grpc/server.rs

# Build dependencies only (this layer will be cached)
RUN cargo build --release --features "grpc,cex" --bin ccxt-grpc-server 2>/dev/null || true

# Remove dummy source
RUN rm -rf src

# Copy actual source code
COPY src ./src
COPY proto ./proto
COPY build.rs ./
COPY examples ./examples
COPY benches ./benches

# Build the actual binary
RUN cargo build --release --features "grpc,cex" --bin ccxt-grpc-server

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    netcat-openbsd \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 appuser

# Set working directory
WORKDIR /app

# Copy the binary from builder
COPY --from=builder /app/target/release/ccxt-grpc-server /app/ccxt-grpc-server

# Use non-root user
USER appuser

# Expose gRPC port
EXPOSE 50051

# Health check using TCP check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD nc -z localhost 50051 || exit 1

# Run the gRPC server
ENTRYPOINT ["/app/ccxt-grpc-server"]
