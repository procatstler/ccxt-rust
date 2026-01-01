# HollaEx WebSocket Implementation

## Overview
Implemented WebSocket client for HollaEx exchange following the ccxt-rust patterns and the JavaScript reference implementation.

## Files Created
- `/Users/kevin/work/github/onlyhyde/trading/ccxt-rust/src/exchanges/foreign/hollaex_ws.rs` - Main WebSocket implementation
- `/Users/kevin/work/github/onlyhyde/trading/ccxt-rust/examples/hollaex_ws_example.rs` - Usage example

## Files Modified
- `/Users/kevin/work/github/onlyhyde/trading/ccxt-rust/src/exchanges/foreign/mod.rs` - Added module declaration and export

## Implementation Details

### WebSocket URL
- Public: `wss://api.hollaex.com/stream`
- Private (with auth): `wss://api.hollaex.com/stream?api-key=...&api-signature=...&api-expires=...`

### Supported Features
1. **Public Streams**:
   - `watch_trades()` - Subscribe to trade events
   - `watch_trades_for_symbols()` - Subscribe to multiple symbols
   - `watch_order_book()` - Subscribe to order book updates
   - `watch_order_book_for_symbols()` - Subscribe to multiple order books

2. **Private Streams** (Placeholder):
   - `watch_balance()` - Requires authentication (not yet implemented)
   - `watch_orders()` - Requires authentication (not yet implemented)

3. **Not Supported**:
   - `watch_ticker()` - HollaEx WS doesn't provide ticker endpoint
   - `watch_ohlcv()` - HollaEx WS doesn't provide OHLCV endpoint

### Message Format

#### Subscribe
```json
{
  "op": "subscribe",
  "args": ["trade:btc-usdt"]
}
```

#### Trade Event
```json
{
  "topic": "trade",
  "action": "partial",
  "symbol": "btc-usdt",
  "data": [
    {
      "size": 0.05,
      "price": 41977.9,
      "side": "buy",
      "timestamp": "2024-01-15T10:30:45.123Z"
    }
  ]
}
```

#### OrderBook Event
```json
{
  "topic": "orderbook",
  "action": "partial",
  "symbol": "btc-usdt",
  "data": {
    "bids": [[41977.9, 0.5], [41977.8, 1.2]],
    "asks": [[41978.0, 0.3], [41978.1, 0.8]],
    "timestamp": "2024-01-15T10:30:45.123Z"
  }
}
```

### Ping/Pong
- Server sends: `{"op": "ping"}`
- Client responds: `{"op": "pong"}` (auto-handled)

### Symbol Format
- Unified: `BTC/USDT`
- HollaEx: `btc-usdt` (lowercase with hyphen)

### Architecture Patterns

1. **ProBit Pattern**: Followed the same structure as `probit_ws.rs`
   - Uses `Arc<RwLock<WebSocketStream>>` for shared WebSocket
   - HashMap for subscription management
   - Separate message processing loop in tokio task
   - OrderBook caching for delta updates

2. **Builder Pattern**: Uses `Trade::new()` with chainable methods
   - `with_timestamp()`
   - `with_side()`

3. **Type Safety**: Uses `rust_decimal::Decimal` for all prices/amounts

4. **Event Wrappers**: Uses standard CCXT-Rust types
   - `WsTradeEvent`
   - `WsOrderBookEvent`
   - `WsMessage` enum

### Key Implementation Details

1. **Decimal Parsing**: Helper function `parse_decimal()` handles:
   - String values: `"123.45"`
   - Float values: `123.45`
   - Integer values: `100`

2. **OrderBook Updates**:
   - Snapshot: `action: "partial"` - Replace entire book
   - Delta: `action: "update"` - Apply incremental changes
   - Cached state maintained in `orderbook_cache`

3. **Error Handling**:
   - Network errors wrapped in `CcxtError::NetworkError`
   - Auth errors wrapped in `CcxtError::AuthenticationError`
   - Unsupported features return `CcxtError::NotSupported`

4. **Subscription Keys**: Format `"{channel}:{symbol}"`
   - `"trades:BTC/USDT"`
   - `"orderbook:BTC/USDT"`

### Authentication (TODO)
Private streams require HMAC authentication with URL parameters:
- `api-key`: API key
- `api-signature`: HMAC-SHA256 signature
- `api-expires`: Expiration timestamp

Not yet implemented - placeholders return `AuthenticationError` or `NotSupported`.

### Testing
Unit tests included for:
- Client creation
- Symbol format conversion (BTC/USDT <-> btc-usdt)
- Decimal parsing

### Usage Example
```rust
use ccxt_rust::exchanges::foreign::HollaexWs;
use ccxt_rust::types::{WsExchange, WsMessage};

let mut ws = HollaexWs::new();
ws.ws_connect().await?;

// Watch trades
let mut trades_rx = ws.watch_trades("BTC/USDT").await?;
while let Some(msg) = trades_rx.recv().await {
    match msg {
        WsMessage::Trade(event) => {
            println!("Trade: {:?}", event);
        }
        _ => {}
    }
}
```

## Verification Steps
1. Added module declaration in `mod.rs`
2. Added public export in `mod.rs`
3. Created comprehensive example file
4. Followed existing code patterns from `probit_ws.rs`
5. Used standard CCXT-Rust types and traits

## Next Steps (Future Enhancement)
1. Implement HMAC authentication for private streams
2. Implement `watch_balance()` with authentication
3. Implement `watch_orders()` with authentication
4. Add integration tests with mock server
5. Performance testing with high-frequency updates
