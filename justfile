# CCXT-Rust Justfile
# Run `just --list` to see all available commands

# Default recipe: show help
default:
    @just --list

# ============== Build ==============

# Build debug
build:
    cargo build

# Build release
build-release:
    cargo build --release

# Build with all features
build-full:
    cargo build --features full

# Build CEX only (default)
build-cex:
    cargo build --features cex

# Build DEX only
build-dex:
    cargo build --no-default-features --features dex

# ============== Test ==============

# Run tests
test:
    cargo test

# Run tests with all features
test-full:
    cargo test --features full

# Run CEX tests
test-cex:
    cargo test --features cex

# Run DEX tests
test-dex:
    cargo test --test dex_tests --features dex

# Run live API tests (requires network)
test-live:
    cargo test --features full live_api -- --ignored --test-threads=1

# Run live DEX tests (requires network)
test-live-dex:
    cargo test --features full live_dex -- --ignored --test-threads=1

# ============== Quality ==============

# Run clippy linter
lint:
    cargo clippy --all-targets --features full -- -D warnings

# Format code
fmt:
    cargo fmt

# Check formatting
fmt-check:
    cargo fmt -- --check

# Run all checks (format, lint, test)
check: fmt-check lint test-full

# ============== Documentation ==============

# Generate documentation
doc:
    cargo doc --features full --no-deps

# Generate and open documentation
doc-open:
    cargo doc --features full --no-deps --open

# ============== Clean ==============

# Clean build artifacts
clean:
    cargo clean

# ============== Development ==============

# Watch and run tests on file changes (requires cargo-watch)
watch-test:
    cargo watch -x 'test --features full'

# Run a specific example
example name:
    cargo run --example {{name}} --features full

# List all examples
examples:
    @ls -1 examples/*.rs | sed 's/examples\///' | sed 's/\.rs//'

# ============== Release ==============

# Check before release
pre-release: fmt-check lint test-full doc
    @echo "✅ All checks passed!"

# Publish to crates.io (dry-run)
publish-dry:
    cargo publish --dry-run

# ============== gRPC ==============

# Build protobuf files
proto:
    cargo build --features grpc

# Run gRPC server
grpc-server:
    cargo run --bin grpc-server --features grpc
