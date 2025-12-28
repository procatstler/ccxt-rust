# Hyperliquid WebSocket Implementation

## Overview

This document describes the Hyperliquid WebSocket implementation for the ccxt-rust library. The implementation provides real-time market data streaming for the Hyperliquid DEX exchange.

## File Location

`src/exchanges/foreign/hyperliquid_ws.rs`

## Features Implemented

### Public Streams

1. **Watch Ticker** (`watch_ticker`)
   - Subscribe to mid price updates via `allMids` channel
   - Provides real-time price data for perpetual futures
   - Symbol format: `BTC/USDC:USDC`

2. **Watch Order Book** (`watch_order_book`)
   - Subscribe to L2 order book updates via `l2Book` channel
   - Returns full order book snapshot with bids and asks
   - Configurable depth (parameter ignored as Hyperliquid provides full book)

3. **Watch Trades** (`watch_trades`)
   - Subscribe to trade stream via `trades` channel
   - Provides trade ID, price, size, side, and timestamp
   - Side: "A" = ask/sell, "B" = bid/buy

4. **Watch OHLCV** (`watch_ohlcv`)
   - Subscribe to candlestick data via `candle` channel
   - Supports all standard timeframes (1m, 5m, 1h, 1d, etc.)
   - Real-time candle updates as they form

## WebSocket URL

- **Production**: `wss://api.hyperliquid.xyz/ws`
- **Testnet**: `wss://api.hyperliquid-testnet.xyz/ws`

## Subscription Format

Hyperliquid uses JSON-based subscription messages:

```json
{
  "method": "subscribe",
  "subscription": {
    "type": "l2Book",
    "coin": "BTC"
  }
}
```

### Subscription Types

1. **allMids**: All mid prices
   ```json
   {
     "method": "subscribe",
     "subscription": { "type": "allMids" }
   }
   ```

2. **l2Book**: Order book for specific coin
   ```json
   {
     "method": "subscribe",
     "subscription": {
       "type": "l2Book",
       "coin": "BTC"
     }
   }
   ```

3. **trades**: Trades for specific coin
   ```json
   {
     "method": "subscribe",
     "subscription": {
       "type": "trades",
       "coin": "BTC"
     }
   }
   ```

4. **candle**: Candle data
   ```json
   {
     "method": "subscribe",
     "subscription": {
       "type": "candle",
       "coin": "BTC",
       "interval": "1m"
     }
   }
   ```

## Symbol Conversion

### Swap Markets (Perpetual Futures)
- **Input**: `BTC/USDC:USDC`
- **Coin**: `BTC`
- **Description**: Extract base currency for swap markets

### Spot Markets
- **Input**: `PURR/USDC`
- **Coin**: `PURR/USDC`
- **Description**: Keep full symbol for spot markets

## Message Types

### Response Structures

1. **Ticker (allMids)**
   ```rust
   {
     "channel": "allMids",
     "data": {
       "mids": {
         "BTC": "50000.0",
         "ETH": "3000.0"
       }
     }
   }
   ```

2. **Order Book (l2Book)**
   ```rust
   {
     "channel": "l2Book",
     "data": {
       "coin": "BTC",
       "time": "1710131872708",
       "levels": [
         [{"px": "68674.0", "sz": "0.97139", "n": 4}],  // bids
         [{"px": "68675.0", "sz": "0.04396", "n": 1}]   // asks
       ]
     }
   }
   ```

3. **Trades**
   ```rust
   {
     "channel": "trades",
     "data": [{
       "coin": "BTC",
       "side": "A",  // "A" = sell, "B" = buy
       "px": "68517.0",
       "sz": "0.005",
       "time": 1710125266669,
       "hash": "0xabc123",
       "tid": 981894269203506
     }]
   }
   ```

4. **Candle**
   ```rust
   {
     "channel": "candle",
     "data": {
       "t": 1710146280000,   // start time
       "T": 1710146339999,   // end time
       "s": "BTC",           // symbol
       "i": "1m",            // interval
       "o": "71400.0",       // open
       "c": "71411.0",       // close
       "h": "71422.0",       // high
       "l": "71389.0",       // low
       "v": "1.20407"        // volume
     }
   }
   ```

## Usage Example

```rust
use ccxt_rust::exchanges::foreign::HyperliquidWs;
use ccxt_rust::types::{Timeframe, WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create client
    let client = HyperliquidWs::new();

    // Watch ticker
    let mut rx = client.watch_ticker("BTC/USDC:USDC").await?;

    while let Some(msg) = rx.recv().await {
        match msg {
            WsMessage::Connected => println!("Connected"),
            WsMessage::Ticker(event) => {
                println!("Price: {:?}", event.ticker.last);
            }
            WsMessage::Error(err) => eprintln!("Error: {}", err),
            _ => {}
        }
    }

    Ok(())
}
```

## Implementation Details

### Architecture

1. **Client Structure**
   - `ws_client`: WebSocket connection manager
   - `subscriptions`: Track active subscriptions
   - `event_tx`: Channel for broadcasting events
   - `sandbox`: Testnet mode flag

2. **Message Processing**
   - JSON parsing with channel-based routing
   - Type-specific handlers for each channel
   - Automatic reconnection on disconnect

3. **Event Distribution**
   - Unbounded channels for event streaming
   - Async task spawning for non-blocking operation
   - Clone-friendly design for multiple subscriptions

### Error Handling

- Network errors: Automatic reconnection
- Parse errors: Logged and skipped
- Invalid messages: Gracefully handled

### Connection Management

- **Auto-reconnect**: Enabled with 5-second interval
- **Max reconnect attempts**: 10
- **Ping interval**: 20 seconds (Hyperliquid keepAlive)
- **Connect timeout**: 30 seconds

## Testing

Run the example:

```bash
cargo run --example hyperliquid_ws_example
```

Run unit tests:

```bash
cargo test hyperliquid_ws
```

## Limitations

1. **Private Streams**: Not yet implemented (requires EIP-712 signing)
2. **Multiple Symbols**: Each subscription requires separate connection
3. **Snapshot vs Delta**: All updates are treated as snapshots

## Future Enhancements

1. Implement private WebSocket streams
   - User orders
   - User trades
   - Account balance updates
   - Position updates

2. Add authentication support
   - EIP-712 signature generation
   - Listen key management

3. Optimize subscriptions
   - Combined stream support
   - Delta updates for order book

## References

- [Hyperliquid WebSocket Documentation](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket)
- [Hyperliquid API Docs](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api)
- [CCXT JavaScript Reference](https://github.com/ccxt/ccxt/blob/master/js/src/pro/hyperliquid.js)

## Version History

- **v1.0.0** (2025-12-26): Initial implementation
  - Public streams: ticker, orderbook, trades, OHLCV
  - WebSocket connection management
  - Message parsing and event distribution
