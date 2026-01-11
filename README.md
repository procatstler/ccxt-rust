# CCXT-Rust

A Rust port of the popular [CCXT](https://github.com/ccxt/ccxt) cryptocurrency exchange trading library.

[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Features

- **100+ Exchanges**: Support for major CEX and DEX platforms
- **REST API**: Fetch markets, tickers, order books, trades, OHLCV data
- **WebSocket**: Real-time streaming for tickers, order books, trades, and candles
- **Unified API**: Consistent interface across all exchanges
- **Async/Await**: Built on Tokio for high-performance async operations
- **Type Safety**: Strongly typed responses with Rust's type system
- **Private APIs**: Authentication support for trading and account management

## Supported Exchanges

### CEX (Centralized Exchanges)

| Exchange | REST | WebSocket | Notes |
|----------|:----:|:---------:|-------|
| Binance | ✅ | ✅ | Spot, Futures, CoinM |
| Bybit | ✅ | ✅ | Spot & Derivatives |
| OKX | ✅ | ✅ | |
| Kraken | ✅ | ✅ | Spot & Futures |
| KuCoin | ✅ | ✅ | Spot & Futures |
| Coinbase | ✅ | ✅ | |
| Gate.io | ✅ | ✅ | |
| Bitget | ✅ | ✅ | |
| MEXC | ✅ | ✅ | |
| HTX (Huobi) | ✅ | ✅ | |
| Upbit | ✅ | ✅ | Korean |
| Bithumb | ✅ | ✅ | Korean |
| **+ 90 more** | | | |

### DEX (Decentralized Exchanges)

| Exchange | REST | WebSocket | Notes |
|----------|:----:|:---------:|-------|
| Hyperliquid | ✅ | ✅ | L1 Perps DEX |
| dYdX v4 | ✅ | ✅ | Cosmos-based |
| Paradex | ✅ | ✅ | StarkNet |
| Apex | ✅ | ✅ | |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
ccxt-rust = { git = "https://github.com/onlyhyde/trading", path = "ccxt-rust" }
tokio = { version = "1.0", features = ["full"] }
```

## Quick Start

### REST API Example

```rust
use ccxt_rust::exchanges::Binance;
use ccxt_rust::types::Exchange;
use ccxt_rust::ExchangeConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create exchange instance
    let config = ExchangeConfig::default();
    let exchange = Binance::new(config)?;

    // Fetch markets
    let markets = exchange.fetch_markets().await?;
    println!("Found {} markets", markets.len());

    // Fetch ticker
    let ticker = exchange.fetch_ticker("BTC/USDT").await?;
    println!("BTC/USDT: {:?}", ticker.last);

    // Fetch order book
    let orderbook = exchange.fetch_order_book("BTC/USDT", Some(10)).await?;
    println!("Best bid: {}", orderbook.bids[0].price);
    println!("Best ask: {}", orderbook.asks[0].price);

    // Fetch OHLCV candles
    let candles = exchange.fetch_ohlcv("BTC/USDT", "1h", None, Some(10)).await?;
    for candle in candles {
        println!("O:{} H:{} L:{} C:{}", candle.open, candle.high, candle.low, candle.close);
    }

    Ok(())
}
```

### WebSocket Example

```rust
use ccxt_rust::exchanges::BinanceWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create WebSocket client
    let client = BinanceWs::new();

    // Subscribe to ticker updates
    let mut ticker_rx = client.watch_ticker("BTC/USDT").await?;

    // Receive real-time updates
    while let Some(msg) = ticker_rx.recv().await {
        match msg {
            WsMessage::Ticker(event) => {
                println!("Ticker: {} @ {:?}", event.symbol, event.ticker.last);
            }
            WsMessage::Error(err) => eprintln!("Error: {err}"),
            _ => {}
        }
    }

    Ok(())
}
```

### Private API (Trading)

```rust
use ccxt_rust::exchanges::Binance;
use ccxt_rust::types::{Exchange, OrderRequest, OrderSide, OrderType};
use ccxt_rust::ExchangeConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create authenticated exchange instance
    let config = ExchangeConfig::default()
        .with_api_key("your-api-key")
        .with_secret("your-secret");
    let exchange = Binance::new(config)?;

    // Fetch account balance
    let balance = exchange.fetch_balance().await?;
    println!("USDT: {:?}", balance.get("USDT"));

    // Create a limit order
    let order = OrderRequest::new("BTC/USDT", OrderType::Limit, OrderSide::Buy)
        .with_amount(0.001)
        .with_price(50000.0);
    let result = exchange.create_order(order).await?;
    println!("Order created: {}", result.id);

    // Fetch open orders
    let orders = exchange.fetch_open_orders(Some("BTC/USDT")).await?;
    println!("Open orders: {}", orders.len());

    Ok(())
}
```

## API Reference

### Exchange Trait (REST)

```rust
pub trait Exchange {
    // Market Data
    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>>;
    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker>;
    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Ticker>>;
    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook>;
    async fn fetch_trades(&self, symbol: &str, since: Option<u64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>>;
    async fn fetch_ohlcv(&self, symbol: &str, timeframe: &str, since: Option<u64>, limit: Option<u32>) -> CcxtResult<Vec<OHLCV>>;

    // Trading (requires authentication)
    async fn fetch_balance(&self) -> CcxtResult<Balances>;
    async fn create_order(&self, order: OrderRequest) -> CcxtResult<Order>;
    async fn cancel_order(&self, id: &str, symbol: Option<&str>) -> CcxtResult<Order>;
    async fn fetch_order(&self, id: &str, symbol: Option<&str>) -> CcxtResult<Order>;
    async fn fetch_open_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>>;
    async fn fetch_closed_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>>;
}
```

### WsExchange Trait (WebSocket)

```rust
pub trait WsExchange {
    // Public Streams
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<Receiver<WsMessage>>;

    // Private Streams (requires authentication)
    async fn watch_balance(&self) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<Receiver<WsMessage>>;
}
```

## Project Structure

```
ccxt-rust/
├── src/
│   ├── lib.rs              # Library entry point
│   ├── client/             # HTTP client, rate limiting
│   ├── crypto/             # Cryptographic utilities (EVM, StarkNet, Cosmos)
│   ├── errors/             # Error types
│   ├── exchanges/
│   │   ├── cex/            # Centralized exchanges (100+)
│   │   └── dex/            # Decentralized exchanges (8)
│   ├── types/              # Common types (Market, Order, etc.)
│   └── utils/              # Utilities (Precise decimal)
├── examples/               # Usage examples
├── tests/                  # Integration tests
└── docs/                   # Documentation
```

## Running Examples

```bash
# REST API example
cargo run --example backpack_example

# WebSocket example
cargo run --example hyperliquid_ws_example

# List all examples
ls examples/
```

## Development

```bash
# Build
cargo build

# Run tests
cargo test

# Run clippy
cargo clippy

# Generate docs
cargo doc --open
```

## Status

| Category | Progress |
|----------|----------|
| REST API | 113/119 (95%) |
| WebSocket | 55/81 (68%) |

See [PORTING_STATUS.md](docs/PORTING_STATUS.md) for detailed exchange support.

## License

MIT License - see [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## Acknowledgments

- [CCXT](https://github.com/ccxt/ccxt) - The original JavaScript/Python library
- All the exchange APIs that make this possible
