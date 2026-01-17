# WebSocket Streaming Guide

Complete guide for real-time market data streaming with ccxt-rust.

## Overview

ccxt-rust provides WebSocket support for **108 exchanges** (100 CEX + 8 DEX) with:
- Real-time ticker, order book, trades, and OHLCV data
- Automatic reconnection with exponential backoff
- Connection health monitoring (ping/pong)
- Private streams for authenticated data (orders, balances, positions)

## Quick Start

### Installation

```toml
[dependencies]
ccxt-rust = { version = "0.1", features = ["cex"] }  # or "dex" or "full"
tokio = { version = "1", features = ["full"] }
```

### Basic Example

```rust
use ccxt_rust::exchanges::cex::BinanceWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = BinanceWs::new();

    // Subscribe to ticker updates
    let mut rx = client.watch_ticker("BTC/USDT").await?;

    while let Some(msg) = rx.recv().await {
        match msg {
            WsMessage::Ticker(event) => {
                println!("BTC price: {:?}", event.ticker.last);
            }
            WsMessage::Error(err) => {
                eprintln!("Error: {}", err);
            }
            _ => {}
        }
    }

    Ok(())
}
```

## Supported Stream Types

### Public Streams (No Authentication Required)

| Method | Description | Returns |
|--------|-------------|---------|
| `watch_ticker(symbol)` | Real-time price updates | `WsTickerEvent` |
| `watch_tickers(symbols)` | Multiple ticker updates | `WsTickerEvent` |
| `watch_order_book(symbol, limit)` | Order book depth | `WsOrderBookEvent` |
| `watch_order_book_for_symbols(symbols, limit)` | Multiple order books | `WsOrderBookEvent` |
| `watch_trades(symbol)` | Market trade executions | `WsTradeEvent` |
| `watch_trades_for_symbols(symbols)` | Multiple symbol trades | `WsTradeEvent` |
| `watch_ohlcv(symbol, timeframe)` | Candlestick data | `WsOhlcvEvent` |

### Private Streams (Authentication Required)

| Method | Description | Returns |
|--------|-------------|---------|
| `watch_balance()` | Account balance changes | `WsBalanceEvent` |
| `watch_orders(symbol)` | Order status updates | `WsOrderEvent` |
| `watch_my_trades(symbol)` | User's trade fills | `WsMyTradeEvent` |
| `watch_positions(symbols)` | Position updates (futures) | `WsPositionEvent` |

### Derivatives Streams

| Method | Description | Returns |
|--------|-------------|---------|
| `watch_mark_price(symbol)` | Mark price updates | `WsTickerEvent` |
| `watch_liquidations(symbol)` | Liquidation events | `WsLiquidationEvent` |

## WsMessage Types

```rust
pub enum WsMessage {
    // Connection lifecycle
    Connected,
    Disconnected,
    Authenticated,

    // Subscription management
    Subscribed { channel: String, symbol: Option<String> },
    Unsubscribed { channel: String, symbol: Option<String> },

    // Market data
    Ticker(WsTickerEvent),
    OrderBook(WsOrderBookEvent),
    Trade(WsTradeEvent),
    Ohlcv(WsOhlcvEvent),

    // Private data
    Order(WsOrderEvent),
    Balance(WsBalanceEvent),
    Position(WsPositionEvent),
    MyTrade(WsMyTradeEvent),

    // Derivatives
    Liquidation(WsLiquidationEvent),

    // Errors
    Error(String),
}
```

## Exchange-Specific Examples

### Binance

```rust
use ccxt_rust::exchanges::cex::BinanceWs;

let client = BinanceWs::new();

// Watch ticker
let mut ticker_rx = client.watch_ticker("BTC/USDT").await?;

// Watch order book with depth limit
let mut book_rx = client.watch_order_book("ETH/USDT", Some(20)).await?;

// Watch trades
let mut trades_rx = client.watch_trades("BTC/USDT").await?;

// Watch OHLCV (candlesticks)
use ccxt_rust::types::Timeframe;
let mut ohlcv_rx = client.watch_ohlcv("BTC/USDT", Timeframe::Minute1).await?;
```

### Binance Futures

```rust
use ccxt_rust::exchanges::cex::BinanceFuturesWs;

let client = BinanceFuturesWs::new();

// Perpetual futures symbol format: BASE/QUOTE:SETTLE
let mut rx = client.watch_ticker("BTC/USDT:USDT").await?;

// Watch mark price
let mut mark_rx = client.watch_mark_price("BTC/USDT:USDT").await?;

// Watch liquidations
let mut liq_rx = client.watch_liquidations("BTC/USDT:USDT").await?;
```

### OKX

```rust
use ccxt_rust::exchanges::cex::OkxWs;

let client = OkxWs::new();

// Spot
let mut rx = client.watch_ticker("BTC/USDT").await?;

// Perpetual swap
let mut swap_rx = client.watch_ticker("BTC/USDT:USDT").await?;
```

### Hyperliquid (DEX)

```rust
use ccxt_rust::exchanges::dex::HyperliquidWs;

// Production
let client = HyperliquidWs::new();

// Testnet
let client = HyperliquidWs::new_sandbox();

let mut rx = client.watch_ticker("BTC/USDC:USDC").await?;
```

### Bybit

```rust
use ccxt_rust::exchanges::cex::BybitWs;

let client = BybitWs::new();
let mut rx = client.watch_order_book("BTC/USDT", Some(50)).await?;
```

## Multiple Subscriptions

### Sequential Subscriptions

```rust
let client = BinanceWs::new();

let mut btc_rx = client.watch_ticker("BTC/USDT").await?;
let mut eth_rx = client.watch_ticker("ETH/USDT").await?;

// Handle in separate tasks
tokio::spawn(async move {
    while let Some(msg) = btc_rx.recv().await {
        // Handle BTC
    }
});

while let Some(msg) = eth_rx.recv().await {
    // Handle ETH
}
```

### Batch Subscription

```rust
let symbols = vec!["BTC/USDT", "ETH/USDT", "SOL/USDT"];
let mut rx = client.watch_tickers(&symbols).await?;

while let Some(msg) = rx.recv().await {
    if let WsMessage::Ticker(event) = msg {
        println!("{}: {:?}", event.ticker.symbol, event.ticker.last);
    }
}
```

## Private Stream Authentication

### Using API Credentials

```rust
use ccxt_rust::exchanges::cex::Binance;

// Create authenticated client
let client = Binance::new_with_credentials(
    "your-api-key",
    "your-secret-key",
);

// Get WebSocket client with authentication
let ws = client.ws();

// Watch private streams
let mut orders_rx = ws.watch_orders(Some("BTC/USDT")).await?;
let mut balance_rx = ws.watch_balance().await?;
```

### Handling Private Events

```rust
while let Some(msg) = orders_rx.recv().await {
    match msg {
        WsMessage::Order(event) => {
            println!("Order update: {:?} - {:?}",
                event.order.id, event.order.status);
        }
        WsMessage::Authenticated => {
            println!("Successfully authenticated");
        }
        _ => {}
    }
}
```

## Connection Management

### Automatic Reconnection

The WebSocket client automatically handles reconnections:

```rust
// Default configuration:
// - Initial delay: 1 second
// - Max delay: 30 seconds
// - Backoff multiplier: 2x
// - Max attempts: 10
// - Jitter: 10%

while let Some(msg) = rx.recv().await {
    match msg {
        WsMessage::Disconnected => {
            println!("Disconnected - auto-reconnecting...");
        }
        WsMessage::Connected => {
            println!("Reconnected successfully");
        }
        _ => {}
    }
}
```

### Health Monitoring

```rust
// Default health check:
// - Ping interval: 30 seconds
// - Pong timeout: 10 seconds
// - Max missed pongs: 3
// - Warning threshold: 45 seconds
```

### Manual Connection Control

```rust
// Check connection status
if client.ws_is_connected().await {
    println!("Connected");
}

// Close connection
client.ws_close().await?;

// Reconnect
client.ws_connect().await?;
```

## Error Handling

### Comprehensive Error Handling

```rust
use ccxt_rust::types::CcxtError;

match client.watch_ticker("BTC/USDT").await {
    Ok(mut rx) => {
        while let Some(msg) = rx.recv().await {
            match msg {
                WsMessage::Error(err) => {
                    eprintln!("Stream error: {}", err);
                    // Decide whether to break or continue
                }
                WsMessage::Disconnected => {
                    println!("Disconnected - waiting for reconnect");
                }
                WsMessage::Ticker(event) => {
                    process_ticker(event);
                }
                _ => {}
            }
        }
    }
    Err(CcxtError::NetworkError { message }) => {
        eprintln!("Network error: {}", message);
    }
    Err(CcxtError::ExchangeError { message }) => {
        eprintln!("Exchange error: {}", message);
    }
    Err(e) => {
        eprintln!("Other error: {}", e);
    }
}
```

## Symbol Formats

### Spot Markets
- Format: `BASE/QUOTE`
- Examples: `BTC/USDT`, `ETH/USD`, `SOL/USDC`

### Perpetual Swaps
- Format: `BASE/QUOTE:SETTLE`
- Examples: `BTC/USDT:USDT`, `ETH/USD:ETH`

### Dated Futures
- Format: `BASE/QUOTE:SETTLE-YYMMDD`
- Examples: `BTC/USDT:USDT-241227`

### Options
- Format: `BASE/QUOTE:SETTLE-YYMMDD-STRIKE-C/P`
- Examples: `BTC/USDT:USDT-241227-100000-C`

## Timeframes

```rust
use ccxt_rust::types::Timeframe;

Timeframe::Second1   // 1s (limited support)
Timeframe::Minute1   // 1m
Timeframe::Minute3   // 3m
Timeframe::Minute5   // 5m
Timeframe::Minute15  // 15m
Timeframe::Minute30  // 30m
Timeframe::Hour1     // 1h
Timeframe::Hour2     // 2h
Timeframe::Hour4     // 4h
Timeframe::Hour6     // 6h
Timeframe::Hour8     // 8h
Timeframe::Hour12    // 12h
Timeframe::Day1      // 1d
Timeframe::Day3      // 3d
Timeframe::Week1     // 1w
Timeframe::Month1    // 1M
```

## Best Practices

### 1. Reuse Clients

```rust
// Good: Create once, reuse
let client = BinanceWs::new();
let rx1 = client.watch_ticker("BTC/USDT").await?;
let rx2 = client.watch_ticker("ETH/USDT").await?;

// Bad: Creating new client for each subscription
let client1 = BinanceWs::new();
let rx1 = client1.watch_ticker("BTC/USDT").await?;
let client2 = BinanceWs::new();  // Unnecessary
let rx2 = client2.watch_ticker("ETH/USDT").await?;
```

### 2. Use Separate Tasks

```rust
let client = BinanceWs::new();

let ticker_rx = client.watch_ticker("BTC/USDT").await?;
let book_rx = client.watch_order_book("BTC/USDT", Some(10)).await?;

// Process in parallel
let ticker_handle = tokio::spawn(process_tickers(ticker_rx));
let book_handle = tokio::spawn(process_orderbook(book_rx));

// Wait for both
tokio::try_join!(ticker_handle, book_handle)?;
```

### 3. Handle Backpressure

```rust
// If processing is slow, messages may queue up
while let Some(msg) = rx.recv().await {
    // Process quickly or spawn task
    tokio::spawn(async move {
        process_message(msg).await;
    });
}
```

### 4. Graceful Shutdown

```rust
use tokio::signal;

let client = BinanceWs::new();
let mut rx = client.watch_ticker("BTC/USDT").await?;

tokio::select! {
    _ = signal::ctrl_c() => {
        println!("Shutting down...");
        client.ws_close().await?;
    }
    msg = rx.recv() => {
        if let Some(m) = msg {
            process(m);
        }
    }
}
```

## Troubleshooting

### Connection Issues

| Problem | Cause | Solution |
|---------|-------|----------|
| Timeout | Network/firewall | Check connectivity, allow WebSocket ports |
| Auth failure | Invalid credentials | Verify API key and secret |
| No data | Wrong symbol | Check symbol format for exchange |
| Frequent disconnects | Rate limiting | Reduce subscription frequency |

### Common Errors

```rust
// Invalid symbol
// Error: "Invalid symbol: BTCUSDT"
// Fix: Use "BTC/USDT" format

// Rate limit exceeded
// Error: "Too many requests"
// Fix: Add delay between subscriptions

// Authentication required
// Error: "Authentication required for private stream"
// Fix: Use authenticated client
```

## Supported Exchanges

### CEX (Centralized Exchanges) - 100 Exchanges

**Tier 1**: Binance, Binance Futures, OKX, Bybit, Coinbase, Kraken, KuCoin, Gate.io, HTX, Bitget, MEXC, Deribit, BitMEX

**Tier 2**: Upbit, Bithumb, Bitfinex, Bitstamp, Gemini, Crypto.com, Phemex, AscendEx, and 80+ more

### DEX (Decentralized Exchanges) - 8 Exchanges

Hyperliquid, dYdX v4, Paradex, Apex, Derive, Defx, WavesExchange, DydxV4

## Next Steps

- [REST API Guide](./ARCHITECTURE.md)
- [Example Code](../examples/)
- [API Reference](https://docs.rs/ccxt-rust)
