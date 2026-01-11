# CCXT-Rust Examples

This directory contains examples demonstrating how to use the ccxt-rust library.

## Quick Start

```bash
# Run any example
cargo run --example <example_name> --features full

# List all examples
ls examples/*.rs
```

## Example Categories

### Getting Started

| Example | Description |
|---------|-------------|
| [basic_usage](basic_usage.rs) | Fundamental operations: creating exchanges, fetching markets, tickers, order books |
| [multi_exchange](multi_exchange.rs) | Working with multiple exchanges simultaneously |
| [trading_operations](trading_operations.rs) | Order creation, cancellation, and management |

### WebSocket Streaming

| Example | Description |
|---------|-------------|
| [websocket_streaming](websocket_streaming.rs) | Generic WebSocket usage patterns |
| [binance_ws_example](binance_ws_example.rs) | Binance real-time data streaming |
| [hyperliquid_ws_example](hyperliquid_ws_example.rs) | Hyperliquid DEX WebSocket |

### Exchange-Specific Examples

#### CEX (Centralized Exchanges)

| Example | Exchange | Features |
|---------|----------|----------|
| [backpack_example](backpack_example.rs) | Backpack | REST API |
| [backpack_ws_example](backpack_ws_example.rs) | Backpack | WebSocket |
| [bullish_ws_example](bullish_ws_example.rs) | Bullish | WebSocket |
| [coinone_ws_example](coinone_ws_example.rs) | Coinone | WebSocket (Korean) |
| [exmo_ws_example](exmo_ws_example.rs) | EXMO | WebSocket |
| [hashkey_ws_example](hashkey_ws_example.rs) | HashKey | WebSocket |
| [hitbtc_ws_example](hitbtc_ws_example.rs) | HitBTC | WebSocket |
| [hollaex_ws_example](hollaex_ws_example.rs) | HollaEx | WebSocket |
| [independentreserve_ws_example](independentreserve_ws_example.rs) | Independent Reserve | WebSocket |
| [krakenfutures_ws_example](krakenfutures_ws_example.rs) | Kraken Futures | WebSocket |
| [kucoinfutures_ws_example](kucoinfutures_ws_example.rs) | KuCoin Futures | WebSocket |
| [onetrading_ws_example](onetrading_ws_example.rs) | OneTrading | WebSocket |
| [toobit_example](toobit_example.rs) | Toobit | REST API |
| [toobit_ws_example](toobit_ws_example.rs) | Toobit | WebSocket |

#### DEX (Decentralized Exchanges)

| Example | Exchange | Features |
|---------|----------|----------|
| [dex_trading](dex_trading.rs) | Hyperliquid | DEX trading operations |
| [hyperliquid_ws_example](hyperliquid_ws_example.rs) | Hyperliquid | WebSocket streaming |

### Configuration and Error Handling

| Example | Description |
|---------|-------------|
| [error_handling](error_handling.rs) | Error types, retry logic, recovery patterns |
| [advanced_config](advanced_config.rs) | Rate limiting, proxies, timeouts, caching |

## Running Examples

### Basic Example

```bash
cargo run --example basic_usage --features cex
```

### WebSocket Example

```bash
cargo run --example websocket_streaming --features cex
```

### DEX Example

```bash
cargo run --example dex_trading --features dex
```

### With All Features

```bash
cargo run --example multi_exchange --features full
```

## Example Patterns

### Creating an Exchange Instance

```rust
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::types::Exchange;

let config = ExchangeConfig::new();
let exchange = Binance::new(config)?;
```

### Fetching Market Data

```rust
// Fetch all markets
let markets = exchange.fetch_markets().await?;

// Fetch ticker
let ticker = exchange.fetch_ticker("BTC/USDT").await?;

// Fetch order book
let orderbook = exchange.fetch_order_book("BTC/USDT", Some(10)).await?;
```

### WebSocket Streaming

```rust
use ccxt_rust::exchanges::cex::BinanceWs;
use ccxt_rust::types::WsExchange;

let ws = BinanceWs::new();
let mut rx = ws.watch_ticker("BTC/USDT").await?;

while let Some(msg) = rx.recv().await {
    println!("Received: {:?}", msg);
}
```

### Error Handling

```rust
use ccxt_rust::CcxtError;

match exchange.fetch_ticker("INVALID/SYMBOL").await {
    Ok(ticker) => println!("Ticker: {:?}", ticker),
    Err(CcxtError::BadSymbol { symbol }) => {
        println!("Invalid symbol: {}", symbol);
    }
    Err(CcxtError::RateLimitExceeded { retry_after_ms, .. }) => {
        if let Some(delay) = retry_after_ms {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
    }
    Err(e) => println!("Error: {}", e),
}
```

### Configuration with Retry and Rate Limiting

```rust
use ccxt_rust::client::{ExchangeConfig, RetryConfig, RateLimiter};

let config = ExchangeConfig::new()
    .with_timeout(30000)
    .with_retry(RetryConfig::default())
    .with_rate_limit(true);
```

## Feature Flags

Examples require specific features to be enabled:

| Feature | Description | Examples |
|---------|-------------|----------|
| `cex` | Centralized exchanges (default) | Most examples |
| `dex` | Decentralized exchanges | `dex_trading`, `hyperliquid_ws_example` |
| `full` | All features | Any example |

## Notes

- **API Keys**: Most examples use public endpoints. For private endpoints (trading), set environment variables:
  ```bash
  export BINANCE_API_KEY="your_key"
  export BINANCE_SECRET="your_secret"
  ```

- **Rate Limiting**: Examples include delays to avoid rate limiting. Adjust as needed.

- **Network**: Examples require internet access to reach exchange APIs.

## Contributing

When adding new examples:

1. Follow the naming convention: `{exchange}_example.rs` or `{exchange}_ws_example.rs`
2. Add comprehensive comments explaining each step
3. Include error handling
4. Update this README with the new example
5. Test with `cargo run --example <name> --features full`
