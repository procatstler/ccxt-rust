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
| `cex` | ✅ | Centralized exchange support (Binance, OKX, etc.) |
| `dex` | ❌ | Decentralized exchange support (Hyperliquid, dYdX, Paradex) + crypto primitives |
| `native` | ✅ | Native Rust build with tokio runtime |
| `wasm` | ❌ | WebAssembly build for browser environments |
| `full` | ❌ | All features enabled (native) |

### Examples

```toml
# CEX only (default, smaller binary)
ccxt-rust = "0.1"

# DEX support (includes EVM, StarkNet, Cosmos crypto)
ccxt-rust = { version = "0.1", features = ["dex"] }

# All features
ccxt-rust = { version = "0.1", features = ["full"] }

# DEX only (no CEX)
ccxt-rust = { version = "0.1", default-features = false, features = ["dex"] }
```

## WebAssembly (Browser) Support

ccxt-rust can be compiled to WebAssembly for use in browsers.

### Build WASM Package

```bash
# Install wasm-pack
cargo install wasm-pack

# Build for web
wasm-pack build --target web --no-default-features --features "wasm,cex"
```

### Browser Usage

```html
<script type="module">
import init, { WasmBinance, WasmUpbit, version } from './pkg/ccxt_rust.js';

async function main() {
    await init();
    console.log('ccxt-rust version:', version());

    // Create exchange instance
    const binance = new WasmBinance();

    // Fetch ticker
    const ticker = await binance.fetchTicker('BTC/USDT');
    console.log('BTC/USDT:', ticker.last);

    // Fetch order book
    const orderbook = await binance.fetchOrderBook('BTC/USDT', 10);
    console.log('Bids:', orderbook.getBids());
    console.log('Asks:', orderbook.getAsks());
}

main();
</script>
```

### Available WASM Exchanges

| Exchange | REST Class | WebSocket Class | REST Methods | WS Methods |
|----------|------------|-----------------|--------------|------------|
| Binance | `WasmBinance` | `WasmBinanceWs` | `fetchTicker`, `fetchOrderBook`, `fetchMarkets`, `fetchTickers`, `fetchTrades`, `fetchOhlcv` | `watchTicker`, `watchOrderBook`, `watchTrades` |
| Upbit | `WasmUpbit` | `WasmUpbitWs` | `fetchTicker`, `fetchOrderBook`, `fetchMarkets`, `fetchTickers`, `fetchTrades`, `fetchOhlcv` | `watchTicker`, `watchOrderBook` |
| Bybit | `WasmBybit` | `WasmBybitWs` | `fetchTicker`, `fetchOrderBook`, `fetchMarkets`, `fetchTickers`, `fetchTrades`, `fetchOhlcv` | `watchTicker`, `watchOrderBook` |
| OKX | `WasmOkx` | - | `fetchTicker`, `fetchOrderBook`, `fetchMarkets`, `fetchTickers`, `fetchTrades`, `fetchOhlcv` | - |
| Kraken | `WasmKraken` | - | `fetchTicker`, `fetchOrderBook`, `fetchMarkets`, `fetchTickers`, `fetchTrades`, `fetchOhlcv` | - |

### WASM WebSocket Example

```javascript
import { WasmBinanceWs } from './pkg/ccxt_rust.js';

// Create WebSocket client
const ws = new WasmBinanceWs();

// Watch ticker with callback
ws.watchTicker('BTC/USDT', (msg) => {
    if (msg.message_type === 'ticker') {
        const data = JSON.parse(msg.data);
        console.log('BTC/USDT:', data.c); // Last price
    } else if (msg.message_type === 'connected') {
        console.log('WebSocket connected');
    }
});
```

### WASM Utilities

```javascript
import {
    hmacSha256,      // HMAC-SHA256 signing
    hmacSha512,      // HMAC-SHA512 signing
    sha256,          // SHA256 hash
    sha512,          // SHA512 hash
    base64Encode,    // Base64 encode
    base64Decode,    // Base64 decode
    urlEncode,       // URL encode
    generateUuid,    // Generate UUID v4
    getCurrentTimestamp,  // Current timestamp (ms)
    getSupportedExchanges // List of supported exchanges
} from './pkg/ccxt_rust.js';
```

> **Note**: WASM builds have some limitations compared to native builds:
> - No private trading APIs (for security reasons)
> - Subject to CORS restrictions (may need a proxy server)

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
