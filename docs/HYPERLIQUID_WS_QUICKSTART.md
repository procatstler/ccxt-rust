# Hyperliquid WebSocket Quick Start Guide

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
ccxt-rust = "0.1"
tokio = { version = "1", features = ["full"] }
```

## Basic Usage

### 1. Import Required Types

```rust
use ccxt_rust::exchanges::foreign::HyperliquidWs;
use ccxt_rust::types::{Timeframe, WsExchange, WsMessage};
```

### 2. Create Client

```rust
// Production
let client = HyperliquidWs::new();

// Testnet
let client = HyperliquidWs::new_sandbox();
```

### 3. Subscribe to Data Streams

#### Watch Ticker (Price Updates)

```rust
let mut rx = client.watch_ticker("BTC/USDC:USDC").await?;

while let Some(msg) = rx.recv().await {
    match msg {
        WsMessage::Ticker(event) => {
            println!("Price: {:?}", event.ticker.last);
        }
        _ => {}
    }
}
```

#### Watch Order Book (L2 Depth)

```rust
let mut rx = client.watch_order_book("BTC/USDC:USDC", Some(10)).await?;

while let Some(msg) = rx.recv().await {
    match msg {
        WsMessage::OrderBook(event) => {
            println!("Bids: {}, Asks: {}",
                event.order_book.bids.len(),
                event.order_book.asks.len()
            );
        }
        _ => {}
    }
}
```

#### Watch Trades (Market Executions)

```rust
let mut rx = client.watch_trades("BTC/USDC:USDC").await?;

while let Some(msg) = rx.recv().await {
    match msg {
        WsMessage::Trade(event) => {
            for trade in &event.trades {
                println!("{} @ {}", trade.amount, trade.price);
            }
        }
        _ => {}
    }
}
```

#### Watch OHLCV (Candlesticks)

```rust
let mut rx = client.watch_ohlcv("BTC/USDC:USDC", Timeframe::Minute1).await?;

while let Some(msg) = rx.recv().await {
    match msg {
        WsMessage::Ohlcv(event) => {
            println!("OHLCV: O:{} H:{} L:{} C:{} V:{}",
                event.ohlcv.open,
                event.ohlcv.high,
                event.ohlcv.low,
                event.ohlcv.close,
                event.ohlcv.volume
            );
        }
        _ => {}
    }
}
```

## Symbol Formats

### Perpetual Futures (Swap)
- Format: `BASE/QUOTE:SETTLE`
- Example: `BTC/USDC:USDC`, `ETH/USDC:USDC`

### Spot Markets
- Format: `BASE/QUOTE`
- Example: `PURR/USDC`, `HYPE/USDC`

## Supported Timeframes

```rust
Timeframe::Minute1    // 1m
Timeframe::Minute3    // 3m
Timeframe::Minute5    // 5m
Timeframe::Minute15   // 15m
Timeframe::Minute30   // 30m
Timeframe::Hour1      // 1h
Timeframe::Hour2      // 2h
Timeframe::Hour4      // 4h
Timeframe::Hour8      // 8h
Timeframe::Hour12     // 12h
Timeframe::Day1       // 1d
Timeframe::Day3       // 3d
Timeframe::Week1      // 1w
Timeframe::Month1     // 1M
```

## Message Types

All WebSocket messages are of type `WsMessage`:

```rust
pub enum WsMessage {
    Connected,              // Connection established
    Disconnected,           // Connection lost
    Ticker(WsTickerEvent),  // Price update
    OrderBook(WsOrderBookEvent),  // Order book update
    Trade(WsTradeEvent),    // Trade execution
    Ohlcv(WsOhlcvEvent),    // Candlestick data
    Error(String),          // Error message
    Authenticated,          // Authentication success
    Subscribed { channel: String, symbol: Option<String> },
    Unsubscribed { channel: String, symbol: Option<String> },
}
```

## Complete Example

```rust
use ccxt_rust::exchanges::foreign::HyperliquidWs;
use ccxt_rust::types::{Timeframe, WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HyperliquidWs::new();

    // Subscribe to ticker
    let mut ticker_rx = client.watch_ticker("BTC/USDC:USDC").await?;

    // Subscribe to trades
    let mut trade_rx = client.watch_trades("BTC/USDC:USDC").await?;

    // Handle ticker updates
    tokio::spawn(async move {
        while let Some(msg) = ticker_rx.recv().await {
            if let WsMessage::Ticker(event) = msg {
                println!("Price: {:?}", event.ticker.last);
            }
        }
    });

    // Handle trade updates
    while let Some(msg) = trade_rx.recv().await {
        if let WsMessage::Trade(event) = msg {
            for trade in &event.trades {
                println!("Trade: {} @ {}", trade.amount, trade.price);
            }
        }
    }

    Ok(())
}
```

## Error Handling

```rust
match client.watch_ticker("BTC/USDC:USDC").await {
    Ok(mut rx) => {
        while let Some(msg) = rx.recv().await {
            match msg {
                WsMessage::Error(err) => {
                    eprintln!("Stream error: {}", err);
                    break;
                }
                WsMessage::Disconnected => {
                    println!("Disconnected - will auto-reconnect");
                }
                WsMessage::Ticker(event) => {
                    println!("Price: {:?}", event.ticker.last);
                }
                _ => {}
            }
        }
    }
    Err(e) => {
        eprintln!("Failed to subscribe: {}", e);
    }
}
```

## Connection Management

The WebSocket client handles:

- ✅ Automatic reconnection on disconnect
- ✅ Ping/pong heartbeat (20s interval)
- ✅ Connection timeout (30s)
- ✅ Max 10 reconnect attempts
- ✅ 5-second reconnect delay

## Performance Tips

1. **Reuse clients**: Create one client instance and reuse for multiple subscriptions
2. **Use async tasks**: Spawn separate tasks for each subscription to handle data concurrently
3. **Buffer management**: Use `UnboundedReceiver` or configure bounded channels based on your needs

## Testing

Run the example:

```bash
cargo run --example hyperliquid_ws_example
```

## Common Issues

### Issue: Connection timeout
**Solution**: Check network connectivity and firewall settings

### Issue: Parse errors
**Solution**: Ensure using correct symbol format (e.g., `BTC/USDC:USDC` for perpetuals)

### Issue: No data received
**Solution**: Verify the symbol exists and is actively trading on Hyperliquid

## Next Steps

- Explore the REST API client for account management
- Check the full documentation for advanced features
- Join the Hyperliquid community for support

## Resources

- [Hyperliquid API Docs](https://hyperliquid.gitbook.io/hyperliquid-docs/)
- [Full Implementation Guide](./HYPERLIQUID_WS_IMPLEMENTATION.md)
- [Example Code](../examples/hyperliquid_ws_example.rs)
