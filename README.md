# CCXT-Rust

A Rust port of the popular [CCXT](https://github.com/ccxt/ccxt) cryptocurrency exchange trading library.

[![CI](https://github.com/onlyhyde/trading/actions/workflows/ci.yml/badge.svg)](https://github.com/onlyhyde/trading/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/ccxt-rust.svg)](https://crates.io/crates/ccxt-rust)
[![Documentation](https://img.shields.io/badge/docs-latest-blue.svg)](https://onlyhyde.github.io/trading/docs/ccxt_rust/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Features

- **100+ Exchanges** - CEX (Binance, Bybit, OKX, ...) and DEX (Hyperliquid, dYdX, Paradex, ...)
- **Unified API** - Consistent interface across all exchanges
- **REST & WebSocket** - Full support for both protocols
- **Async/Await** - Built on Tokio for high-performance operations
- **Type Safety** - Strongly typed with Rust's type system

## Supported Exchanges

| Type | Exchanges |
|------|-----------|
| CEX | Binance, Bybit, OKX, Kraken, KuCoin, Coinbase, Gate.io, Bitget, MEXC, HTX, Upbit, Bithumb, +90 more |
| DEX | Hyperliquid, dYdX v4, Paradex, Apex, Defx, Derive, WavesExchange |

See [docs/exchanges.md](docs/exchanges.md) for the full list.

## Installation

```toml
[dependencies]
ccxt-rust = "0.1"
tokio = { version = "1.0", features = ["full"] }
```

### Feature Flags

| Feature | Default | Description |
|---------|:-------:|-------------|
| `cex` | ✅ | Centralized exchanges |
| `dex` | ❌ | Decentralized exchanges + crypto primitives |
| `full` | ❌ | All features |

## Quick Start

```rust
use ccxt_rust::exchanges::Binance;
use ccxt_rust::types::Exchange;
use ccxt_rust::ExchangeConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let exchange = Binance::new(ExchangeConfig::default())?;

    // Fetch ticker
    let ticker = exchange.fetch_ticker("BTC/USDT").await?;
    println!("BTC/USDT: {:?}", ticker.last);

    // Fetch order book
    let orderbook = exchange.fetch_order_book("BTC/USDT", Some(5)).await?;
    println!("Best bid: {}, Best ask: {}",
        orderbook.bids[0].price,
        orderbook.asks[0].price
    );

    Ok(())
}
```

See [examples/](examples/) for more examples including WebSocket and trading.

## Documentation

- [API Documentation](https://onlyhyde.github.io/trading/docs/ccxt_rust/)
- [Examples](examples/README.md)
- [Architecture](docs/ARCHITECTURE.md)

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

[MIT](LICENSE)
