# Contributing to CCXT-Rust

Thank you for your interest in contributing to CCXT-Rust! This document provides guidelines and information for contributors.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Making Changes](#making-changes)
- [Adding a New Exchange](#adding-a-new-exchange)
- [Testing](#testing)
- [Pull Request Process](#pull-request-process)
- [Style Guide](#style-guide)

## Code of Conduct

By participating in this project, you agree to maintain a respectful and inclusive environment. Please be considerate of others and focus on constructive collaboration.

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/YOUR_USERNAME/trading.git`
3. Navigate to ccxt-rust: `cd trading/ccxt-rust`
4. Create a new branch: `git checkout -b feature/your-feature-name`

## Development Setup

### Prerequisites

- Rust 1.70 or higher
- Cargo (comes with Rust)

### Build

```bash
# Build with default features (CEX only)
cargo build

# Build with all features
cargo build --features full

# Build in release mode
cargo build --release
```

### Run Tests

```bash
# Run all tests
cargo test --features full

# Run specific test
cargo test test_name --features full

# Run with output
cargo test --features full -- --nocapture
```

### Code Quality

```bash
# Run clippy lints
cargo clippy --features full -- -D warnings

# Format code
cargo fmt

# Check formatting
cargo fmt -- --check

# Build documentation
cargo doc --features full --no-deps
```

## Making Changes

### Branch Naming

- `feature/` - New features
- `fix/` - Bug fixes
- `docs/` - Documentation changes
- `refactor/` - Code refactoring
- `test/` - Test additions or modifications

### Commit Messages

Follow conventional commits format:

```
type(scope): description

[optional body]

[optional footer]
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`

Examples:
```
feat(binance): add spot margin trading support
fix(websocket): handle reconnection on network failure
docs(readme): update installation instructions
```

## Adding a New Exchange

### 1. Determine Exchange Type

- **CEX**: Add to `src/exchanges/cex/`
- **DEX**: Add to `src/exchanges/dex/`

### 2. Create Exchange File

```rust
// src/exchanges/cex/newexchange.rs

use crate::client::{ExchangeConfig, HttpClient};
use crate::errors::CcxtResult;
use crate::types::*;
use async_trait::async_trait;

pub struct NewExchange {
    client: HttpClient,
    // ... other fields
}

impl NewExchange {
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        // Implementation
    }
}

#[async_trait]
impl Exchange for NewExchange {
    fn id(&self) -> ExchangeId {
        ExchangeId::NewExchange
    }

    fn name(&self) -> &str {
        "New Exchange"
    }

    // Implement required methods...
}
```

### 3. Add WebSocket Support (Optional)

Create `src/exchanges/cex/newexchange_ws.rs` implementing `WsExchange` trait.

### 4. Update Module Exports

```rust
// src/exchanges/cex/mod.rs
mod newexchange;
pub use newexchange::NewExchange;

// If WebSocket:
mod newexchange_ws;
pub use newexchange_ws::NewExchangeWs;
```

### 5. Add ExchangeId

```rust
// src/types/exchange.rs
pub enum ExchangeId {
    // ...existing exchanges...
    NewExchange,
}
```

### 6. Add Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_newexchange_creation() {
        let config = ExchangeConfig::new();
        let exchange = NewExchange::new(config);
        assert!(exchange.is_ok());
    }

    #[tokio::test]
    async fn test_newexchange_features() {
        // Test exchange features
    }
}
```

### 7. Add Example (Optional)

Create `examples/newexchange_example.rs` demonstrating usage.

## Testing

### Test Categories

1. **Unit Tests**: Test individual components
2. **Integration Tests**: Test in `tests/` directory
3. **Live API Tests**: Test against real APIs (ignored by default)

### Running Live Tests

```bash
# CEX live tests
cargo test --features full live_api -- --ignored --test-threads=1

# DEX live tests
cargo test --features full live_dex -- --ignored --test-threads=1
```

### Writing Tests

- Test both success and error cases
- Use meaningful test names
- Add comments for complex test logic
- Mock external dependencies when possible

## Pull Request Process

### Before Submitting

1. Ensure all tests pass: `cargo test --features full`
2. Run clippy: `cargo clippy --features full -- -D warnings`
3. Format code: `cargo fmt`
4. Update documentation if needed
5. Add tests for new functionality

### PR Description

Include:
- Summary of changes
- Related issue number (if applicable)
- Breaking changes (if any)
- Testing performed

### Review Process

1. Automated CI checks must pass
2. At least one maintainer review required
3. Address all review comments
4. Squash commits if requested

## Style Guide

### Rust Style

- Follow Rust API guidelines
- Use `rustfmt` for formatting
- Prefer explicit types over inference when it aids readability
- Document public APIs with doc comments

### Documentation

```rust
/// Brief description of the function.
///
/// More detailed explanation if needed.
///
/// # Arguments
///
/// * `param` - Description of parameter
///
/// # Returns
///
/// Description of return value
///
/// # Errors
///
/// Description of possible errors
///
/// # Examples
///
/// ```rust
/// let result = function(param);
/// ```
pub fn function(param: Type) -> Result<Type, Error> {
    // Implementation
}
```

### Error Handling

- Use `CcxtError` for exchange-related errors
- Provide meaningful error messages
- Include context in error messages

### Async Code

- Use `async_trait` for async trait methods
- Prefer `tokio` runtime primitives
- Handle timeouts and cancellation properly

## Questions?

- Open an issue for bugs or feature requests
- Check existing issues before creating new ones
- Be specific and provide reproduction steps for bugs

Thank you for contributing!
