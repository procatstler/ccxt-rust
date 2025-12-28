# HitBTC WebSocket Implementation

## Overview
This document describes the HitBTC WebSocket implementation added to ccxt-rust.

## Implementation Details

### File Structure
- **Main Implementation**: `/src/exchanges/foreign/hitbtc_ws.rs`
- **Example**: `/examples/hitbtc_ws_example.rs`
- **Exports**: Added to `mod.rs` and `exchanges/mod.rs`

### WebSocket URLs
- **Public**: `wss://api.hitbtc.com/api/3/ws/public`
- **Private**: `wss://api.hitbtc.com/api/3/ws/trading` (not yet implemented)

### Implemented Features

#### Public Streams
1. **watch_ticker(symbol)** - Subscribe to ticker updates
   - Channel: `ticker/1s`
   - Provides: last price, bid/ask, volume, high/low

2. **watch_tickers(symbols)** - Subscribe to multiple tickers
   - Channel: `ticker/1s`
   - Supports batch subscription

3. **watch_order_book(symbol, limit)** - Subscribe to order book updates
   - Channel: `orderbook/full`
   - Real-time bid/ask updates with sequence numbers

4. **watch_trades(symbol)** - Subscribe to recent trades
   - Channel: `trades`
   - Provides: price, quantity, side, timestamp

5. **watch_ohlcv(symbol, timeframe)** - Subscribe to candlestick data
   - Channel: `candles/{period}` (e.g., `candles/M1`)
   - Supported timeframes: M1, M3, M5, M15, M30, H1, H4, D1, D7, 1M

#### Private Streams (Not Yet Implemented)
- watch_balance()
- watch_orders()
- watch_my_trades()
- ws_authenticate()

### Subscription Format
HitBTC uses JSON-RPC style subscriptions:

```json
{
  "method": "subscribe",
  "ch": "ticker/1s",
  "params": {
    "symbols": ["BTCUSDT"]
  },
  "id": 1
}
```

### Symbol Format
- **Input**: `BTC/USDT` (unified format)
- **WebSocket**: `BTCUSDT` (no separator)
- **Conversion**: Automatic via `format_symbol()` and `to_unified_symbol()`

### Message Structure

#### Ticker Data
```rust
struct HitbtcTickerData {
    symbol: String,
    timestamp: Option<String>,
    last: Option<String>,
    open: Option<String>,
    high: Option<String>,
    low: Option<String>,
    volume: Option<String>,
    volume_quote: Option<String>,
    best_bid_price: Option<String>,
    best_bid_size: Option<String>,
    best_ask_price: Option<String>,
    best_ask_size: Option<String>,
}
```

#### Order Book Data
```rust
struct HitbtcOrderBookData {
    symbol: String,
    timestamp: Option<String>,
    sequence: Option<i64>,
    ask: Vec<HitbtcOrderBookLevel>,
    bid: Vec<HitbtcOrderBookLevel>,
}
```

#### Trade Data
```rust
struct HitbtcTradeData {
    id: i64,
    symbol: String,
    price: String,
    quantity: String,
    side: String,
    timestamp: Option<String>,
}
```

#### Candle Data
```rust
struct HitbtcCandleData {
    symbol: String,
    timestamp: Option<String>,
    open: String,
    close: String,
    min: String,
    max: String,
    volume: Option<String>,
    volume_quote: Option<String>,
}
```

## Usage Example

```rust
use ccxt_rust::exchanges::HitbtcWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create WebSocket client
    let hitbtc_ws = HitbtcWs::new();

    // Subscribe to ticker
    let mut ticker_rx = hitbtc_ws.watch_ticker("BTC/USDT").await?;

    // Listen for updates
    while let Some(msg) = ticker_rx.recv().await {
        match msg {
            WsMessage::Connected => println!("Connected"),
            WsMessage::Ticker(event) => {
                println!("Ticker: {} - Last: {:?}", event.symbol, event.ticker.last);
            }
            WsMessage::Error(err) => eprintln!("Error: {}", err),
            _ => {}
        }
    }

    Ok(())
}
```

## Testing

### Compile Test
```bash
cargo build --lib
```

### Run Example
```bash
cargo run --example hitbtc_ws_example
```

## Architecture

### HitbtcWs Struct
```rust
pub struct HitbtcWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: Arc<AtomicI64>,
}
```

### Key Features
1. **Atomic Request IDs**: Thread-safe request ID generation
2. **Subscription Tracking**: HashMap to track active subscriptions
3. **Event Broadcasting**: mpsc channel for event distribution
4. **Auto-reconnection**: Configured via WsConfig
5. **Error Handling**: Proper JSON-RPC error parsing

### Message Processing Flow
1. WebSocket receives message
2. Parse JSON-RPC response
3. Identify message type (ticker, orderbook, trade, candle)
4. Convert to unified format
5. Broadcast via event channel

## Timeframe Mapping

| CCXT Timeframe | HitBTC Period |
|----------------|---------------|
| Minute1        | M1            |
| Minute3        | M3            |
| Minute5        | M5            |
| Minute15       | M15           |
| Minute30       | M30           |
| Hour1          | H1            |
| Hour4          | H4            |
| Day1           | D1            |
| Week1          | D7            |
| Month1         | 1M            |

## Known Limitations

1. **Private Streams**: Not yet implemented
   - Authentication
   - Balance updates
   - Order updates
   - Trade history

2. **Advanced Features**: Not implemented
   - Order book snapshots vs updates differentiation
   - Reconnection state recovery
   - Subscription management (unsubscribe)

## Future Enhancements

1. Implement private WebSocket streams
2. Add authentication via API key
3. Implement order/balance streaming
4. Add subscription state persistence
5. Implement graceful unsubscribe
6. Add connection health monitoring

## References

- HitBTC API Documentation: https://api.hitbtc.com
- HitBTC WebSocket API v3: https://api.hitbtc.com/api/3/ws/public
- Existing REST Implementation: `/src/exchanges/foreign/hitbtc.rs`
- Similar Implementations: `binance_ws.rs`, `gate_ws.rs`
