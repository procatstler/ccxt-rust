---
name: Bug Report
about: Report a bug in ccxt-rust
title: '[BUG] '
labels: bug
assignees: ''
---

## Bug Description

A clear and concise description of the bug.

## Exchange

- Exchange Name: (e.g., Binance, OKX, Hyperliquid)
- Exchange Type: CEX / DEX
- API Type: REST / WebSocket

## To Reproduce

Steps to reproduce the behavior:

1. Create exchange instance with '...'
2. Call method '...'
3. See error

## Code Example

```rust
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::types::Exchange;

// Minimal reproducible example
let config = ExchangeConfig::new();
let exchange = Binance::new(config)?;
// ... code that causes the bug
```

## Expected Behavior

A clear description of what you expected to happen.

## Actual Behavior

What actually happened, including any error messages.

## Error Output

```
// Paste the full error message or stack trace here
```

## Environment

- OS: (e.g., macOS 14.0, Ubuntu 22.04, Windows 11)
- Rust Version: (output of `rustc --version`)
- ccxt-rust Version: (from Cargo.toml)
- Feature Flags: (e.g., `cex`, `dex`, `full`)

## Additional Context

Add any other context about the problem here (screenshots, logs, etc.)

## Checklist

- [ ] I have searched existing issues to ensure this is not a duplicate
- [ ] I have tested with the latest version
- [ ] I have provided a minimal reproducible example
