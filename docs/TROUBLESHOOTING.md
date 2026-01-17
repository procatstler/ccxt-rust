# Troubleshooting Guide

Common issues and solutions when using ccxt-rust.

## Table of Contents

- [Installation Issues](#installation-issues)
- [Connection Problems](#connection-problems)
- [Authentication Errors](#authentication-errors)
- [Symbol and Market Errors](#symbol-and-market-errors)
- [Order Execution Issues](#order-execution-issues)
- [WebSocket Issues](#websocket-issues)
- [Performance Issues](#performance-issues)
- [Build Errors](#build-errors)

---

## Installation Issues

### Feature Flag Errors

**Problem**: Compilation error about missing types or modules

```
error[E0433]: failed to resolve: could not find `dex` in `exchanges`
```

**Solution**: Enable the correct feature flag

```toml
# For CEX only (default)
ccxt-rust = "0.1"

# For DEX support
ccxt-rust = { version = "0.1", features = ["dex"] }

# For advanced crypto (ECDSA, Ed25519)
ccxt-rust = { version = "0.1", features = ["advanced-crypto"] }

# For all features
ccxt-rust = { version = "0.1", features = ["full"] }
```

### Dependency Conflicts

**Problem**: Version conflicts with other crates

**Solution**: Check your `Cargo.lock` and use compatible versions

```toml
# Ensure compatible tokio version
tokio = { version = "1.0", features = ["full"] }

# Ensure compatible serde version
serde = { version = "1.0", features = ["derive"] }
```

---

## Connection Problems

### Timeout Errors

**Problem**: `NetworkError: Connection timeout`

**Causes**:
1. Network connectivity issues
2. Firewall blocking outbound connections
3. Exchange API server issues

**Solutions**:

```rust
// 1. Check network connectivity first
// 2. Verify you can reach the exchange
// 3. Try with longer timeout

use std::time::Duration;

// Configure longer timeout
let config = ExchangeConfig {
    timeout: Duration::from_secs(60),
    ..Default::default()
};
```

### SSL/TLS Errors

**Problem**: `NetworkError: SSL handshake failed`

**Solutions**:
1. Update system CA certificates
2. Check system time is correct
3. Try with rustls instead of native-tls

```toml
# In Cargo.toml, ensure rustls is used
ccxt-rust = { version = "0.1", default-features = false, features = ["cex"] }
```

### Rate Limiting

**Problem**: `ExchangeError: Too many requests` or `429 status code`

**Solution**: Implement rate limiting

```rust
use std::time::Duration;
use tokio::time::sleep;

// Add delay between requests
for symbol in symbols {
    let ticker = exchange.fetch_ticker(&symbol).await?;
    sleep(Duration::from_millis(100)).await;  // Rate limit delay
}
```

---

## Authentication Errors

### Invalid API Key

**Problem**: `AuthenticationError: Invalid API key`

**Causes**:
1. Incorrect API key
2. API key not activated
3. IP whitelist restrictions

**Solutions**:

```rust
// Verify credentials
let exchange = Binance::new_with_credentials(
    "your-api-key",      // Check for typos, extra spaces
    "your-secret-key",
);

// Test authentication
match exchange.fetch_balance().await {
    Ok(balance) => println!("Authenticated: {:?}", balance),
    Err(e) => eprintln!("Auth error: {}", e),
}
```

### Signature Errors

**Problem**: `AuthenticationError: Invalid signature`

**Causes**:
1. Incorrect secret key
2. System time out of sync
3. Missing required parameters

**Solutions**:

```rust
// 1. Verify your secret key is correct
// 2. Sync system time with NTP server
// 3. Check exchange-specific requirements

// For exchanges requiring passphrase (e.g., OKX)
let exchange = Okx::new_with_credentials(
    "api-key",
    "secret-key",
    Some("passphrase"),  // Required for OKX
);
```

### Permission Errors

**Problem**: `AuthenticationError: Permission denied`

**Causes**:
- API key doesn't have required permissions
- Trading disabled on account
- Futures/margin not enabled

**Solutions**:
1. Check API key permissions in exchange dashboard
2. Enable required features (spot, futures, margin)
3. Complete exchange KYC if required

---

## Symbol and Market Errors

### Invalid Symbol Format

**Problem**: `ExchangeError: Invalid symbol: BTCUSDT`

**Solution**: Use unified symbol format

```rust
// Correct formats:
let spot = "BTC/USDT";           // Spot market
let perp = "BTC/USDT:USDT";      // Perpetual swap
let future = "BTC/USDT:USDT-241227";  // Dated future
let option = "BTC/USDT:USDT-241227-100000-C";  // Call option

// Wrong formats:
// "BTCUSDT"     - Missing slash
// "BTC-USDT"    - Wrong separator
// "btc/usdt"    - Wrong case (some exchanges)
```

### Symbol Not Found

**Problem**: `ExchangeError: Symbol not found`

**Solution**: Load markets and verify symbol exists

```rust
// Load markets first
exchange.load_markets(false).await?;

// Check if symbol exists
let markets = exchange.fetch_markets().await?;
for market in &markets {
    if market.symbol.contains("BTC") {
        println!("{}", market.symbol);
    }
}

// Use the correct symbol from the list
```

### Market Not Active

**Problem**: Trading on inactive/delisted market

**Solution**: Check market status

```rust
let markets = exchange.fetch_markets().await?;
let btc_market = markets.iter()
    .find(|m| m.symbol == "BTC/USDT")
    .expect("Market not found");

if !btc_market.active {
    eprintln!("Market is not active for trading");
}
```

---

## Order Execution Issues

### Insufficient Balance

**Problem**: `ExchangeError: Insufficient balance`

**Solution**: Check balance before ordering

```rust
let balance = exchange.fetch_balance().await?;

if let Some(usdt) = balance.get("USDT") {
    if usdt.free < order_cost {
        eprintln!("Insufficient balance: have {}, need {}",
            usdt.free, order_cost);
    }
}
```

### Minimum Order Size

**Problem**: `ExchangeError: Order quantity too small`

**Solution**: Check market limits

```rust
exchange.load_markets(false).await?;
let market = exchange.market("BTC/USDT")?;

println!("Min order amount: {:?}", market.limits.amount.min);
println!("Min order cost: {:?}", market.limits.cost.min);

// Ensure order meets minimums
if amount < market.limits.amount.min.unwrap_or_default() {
    eprintln!("Order below minimum amount");
}
```

### Price Precision Errors

**Problem**: `ExchangeError: Invalid price precision`

**Solution**: Round to market precision

```rust
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

let market = exchange.market("BTC/USDT")?;
let precision = market.precision.price.unwrap_or(2);

// Round price to correct precision
let price = Decimal::from_str("50000.123456")?;
let rounded = price.round_dp(precision as u32);
```

---

## WebSocket Issues

### Connection Drops

**Problem**: WebSocket disconnects frequently

**Causes**:
1. Network instability
2. Server-side disconnects
3. Missing heartbeat

**Solution**: Handle reconnection

```rust
let mut rx = client.watch_ticker("BTC/USDT").await?;

loop {
    match rx.recv().await {
        Some(WsMessage::Disconnected) => {
            println!("Disconnected, waiting for reconnect...");
            // Auto-reconnect is handled by the client
        }
        Some(WsMessage::Connected) => {
            println!("Connected!");
        }
        Some(msg) => {
            process_message(msg);
        }
        None => {
            // Channel closed, need to resubscribe
            rx = client.watch_ticker("BTC/USDT").await?;
        }
    }
}
```

### No Data Received

**Problem**: Subscribed but receiving no messages

**Causes**:
1. Wrong symbol format
2. Market not active
3. Subscription failed

**Solution**:

```rust
let mut rx = client.watch_ticker("BTC/USDT").await?;

while let Some(msg) = rx.recv().await {
    match msg {
        WsMessage::Subscribed { channel, symbol } => {
            println!("Subscribed to {} {:?}", channel, symbol);
        }
        WsMessage::Error(err) => {
            eprintln!("Subscription error: {}", err);
            break;
        }
        _ => {}
    }
}
```

### Message Parsing Errors

**Problem**: `Error: Failed to parse message`

**Causes**:
1. Exchange API changed
2. Unexpected message format

**Solution**: Log raw messages for debugging

```rust
// Enable debug logging
std::env::set_var("RUST_LOG", "ccxt_rust=debug");
tracing_subscriber::fmt::init();

// Or handle gracefully
match msg {
    WsMessage::Error(e) if e.contains("parse") => {
        eprintln!("Parse error (exchange may have changed API): {}", e);
    }
    _ => {}
}
```

---

## Performance Issues

### Slow Response Times

**Problem**: API calls taking too long

**Solutions**:

```rust
// 1. Use connection pooling (default)
// 2. Enable keep-alive
// 3. Use regional endpoints if available

// For Binance, use closest region
let exchange = Binance::new_with_config(BinanceConfig {
    base_url: "https://api1.binance.com",  // US server
    ..Default::default()
});
```

### High Memory Usage

**Problem**: Memory grows over time

**Solutions**:
1. Don't store all historical data
2. Use streaming instead of batch fetches
3. Process and discard old data

```rust
// Use streaming for real-time data
let mut rx = client.watch_trades("BTC/USDT").await?;

while let Some(msg) = rx.recv().await {
    if let WsMessage::Trade(event) = msg {
        // Process immediately, don't accumulate
        process_trade(&event);
    }
}
```

---

## Build Errors

### OpenSSL Errors (Linux)

**Problem**: `failed to run custom build command for openssl-sys`

**Solution**:

```bash
# Ubuntu/Debian
sudo apt-get install libssl-dev pkg-config

# Fedora/RHEL
sudo dnf install openssl-devel

# Or use rustls (no system deps)
# Already default in ccxt-rust
```

### Linker Errors (macOS)

**Problem**: `ld: library not found for -lssl`

**Solution**:

```bash
# Install OpenSSL via Homebrew
brew install openssl

# Set environment variables
export OPENSSL_DIR=$(brew --prefix openssl)
```

### Windows Build Errors

**Problem**: Build fails on Windows

**Solution**:

```bash
# Install Visual Studio Build Tools
# Or use rustls backend (default)

# Ensure correct toolchain
rustup default stable-msvc
```

---

## Getting Help

### Debug Logging

Enable detailed logging:

```rust
// In your main.rs
std::env::set_var("RUST_LOG", "ccxt_rust=debug");
tracing_subscriber::fmt::init();
```

### Reporting Issues

When reporting issues, include:
1. Rust version (`rustc --version`)
2. ccxt-rust version
3. Feature flags used
4. Exchange name
5. Error message (full)
6. Minimal reproduction code

### Resources

- [GitHub Issues](https://github.com/onlyhyde/trading/issues)
- [Documentation](https://docs.rs/ccxt-rust)
- [Examples](../examples/)
- [Architecture Guide](./ARCHITECTURE.md)
