# Testing Guide

This document describes the testing strategy and how to run various types of tests in ccxt-rust.

## Test Categories

### 1. Unit Tests (~1,335 tests)

Unit tests verify individual components without making external calls.

```bash
# Run all unit tests
cargo test

# Run with all features
cargo test --features full

# Run specific test module
cargo test --test exchange_trait_tests
```

### 2. Integration Tests (~47 tests)

Integration tests verify component interactions.

```bash
# Run integration tests
cargo test --features full --test '*'
```

### 3. Live API Tests (28 tests)

Live API tests make actual calls to exchange APIs. These are marked with `#[ignore]` to prevent accidental execution in CI.

#### CEX Live Tests (17 tests)

| Exchange | Tests |
|----------|-------|
| Binance | fetch_markets, fetch_ticker, fetch_order_book, fetch_ohlcv, fetch_trades |
| OKX | fetch_markets, fetch_ticker, fetch_order_book |
| Bybit | fetch_markets, fetch_ticker, fetch_order_book |
| Kraken | fetch_markets, fetch_ticker |
| Upbit | fetch_markets, fetch_ticker |
| Bithumb | fetch_markets |
| Cross-Exchange | price_comparison (BTC/USDT across Binance, OKX, Bybit) |

```bash
# Run CEX live tests
cargo test --features full live_api -- --ignored --test-threads=1
```

#### DEX Live Tests (11 tests)

| Exchange | Tests |
|----------|-------|
| Hyperliquid | fetch_markets, fetch_ticker, fetch_order_book, fetch_ohlcv, fetch_funding_rate |
| dYdX v4 | fetch_markets, fetch_ticker, fetch_order_book |
| Paradex | fetch_markets, fetch_ticker |
| Performance | latency measurement |

```bash
# Run DEX live tests
cargo test --features full live_dex -- --ignored --test-threads=1
```

### 4. Documentation Tests (2 tests)

Doc tests verify code examples in documentation.

```bash
cargo test --doc --features full
```

## Running Tests

### Quick Test (Default)

```bash
cargo test
```

### Full Test Suite

```bash
cargo test --features full
```

### Specific Feature Tests

```bash
# CEX only
cargo test --features cex

# DEX only (no default features)
cargo test --no-default-features --features dex
```

### Live API Tests

**Important**: Live tests make real API calls. Use responsibly to avoid rate limiting.

```bash
# All live tests (CEX + DEX)
cargo test --features full -- --ignored --test-threads=1

# CEX only
cargo test --features full live_api -- --ignored --test-threads=1

# DEX only
cargo test --features full live_dex -- --ignored --test-threads=1

# Single exchange
cargo test --features full binance_live -- --ignored --test-threads=1
```

## Test Configuration

### Rate Limiting

Live tests include built-in delays to avoid rate limiting:
- CEX tests: 500ms between calls
- DEX tests: 1000ms between calls (blockchain latency)

### Test Timeouts

- Standard tests: Default cargo timeout
- Live API tests: 10 minutes (CI configuration)

## CI/CD Integration

### Automatic Tests (on push/PR)

- Unit tests
- Integration tests
- Clippy lints
- Format check
- Documentation build
- Feature combination matrix

### Manual Tests (workflow_dispatch)

- Live CEX API tests
- Live DEX API tests

Trigger manual tests via GitHub Actions "Run workflow" button.

## Writing Tests

### Unit Test Example

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_parsing() {
        let symbol = "BTC/USDT";
        assert!(symbol.contains("/"));
    }
}
```

### Async Test Example

```rust
#[tokio::test]
async fn test_async_operation() {
    let result = some_async_fn().await;
    assert!(result.is_ok());
}
```

### Live Test Example

```rust
#[tokio::test]
#[ignore]  // Important: mark as ignored for CI
async fn test_live_api_call() {
    let exchange = Binance::new(ExchangeConfig::new()).unwrap();
    let markets = exchange.fetch_markets().await;
    assert!(markets.is_ok());
}
```

## Test Coverage

| Category | Count | Status |
|----------|-------|--------|
| Total Tests | ~1,461 | Passing |
| Unit Tests | ~1,335 | Passing |
| Integration Tests | ~126 | Passing |
| Live API Tests | 28 | Ignored (manual) |
| Doc Tests | 11 | Ignored |

> **Last verified**: 2026년 1월

## Troubleshooting

### "Too many open files" Error

Increase ulimit:
```bash
ulimit -n 4096
```

### Rate Limit Errors in Live Tests

- Wait before retrying
- Use `--test-threads=1` to serialize requests
- Increase delay between calls if needed

### Network Errors

Live tests may fail due to:
- Network issues
- Exchange maintenance
- API changes
- Geographic restrictions

Use `continue-on-error: true` in CI for live tests.
