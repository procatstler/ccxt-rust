# P2B WebSocket Implementation

## Overview
Implemented WebSocket support for P2B exchange (p2pb2b.com) following the CCXT reference implementation.

## File Location
- `/Users/kevin/work/github/onlyhyde/trading/ccxt-rust/src/exchanges/foreign/p2b_ws.rs`

## Features Implemented

### Connection Management
- WebSocket URL: `wss://apiws.p2pb2b.com/`
- Automatic ping/pong keep-alive (30-second interval)
- Connection lifecycle management (connect, close, is_connected)

### Supported Methods

#### 1. watchTicker
- Method: `state.subscribe` or `price.subscribe`
- Subscribe to ticker updates for a single symbol
- Returns: WsTickerEvent with high, low, last, open, close, volume data

#### 2. watchTickers
- Method: `state.subscribe`
- Subscribe to ticker updates for multiple symbols
- Batch subscription support

#### 3. watchTrades
- Method: `deals.subscribe`
- Subscribe to trade updates for a single symbol
- Returns: WsTradeEvent with trade id, price, amount, side, timestamp

#### 4. watchTradesForSymbols
- Method: `deals.subscribe`
- Subscribe to trades for multiple symbols simultaneously
- Batch subscription support

#### 5. watchOrderBook
- Method: `depth.subscribe`
- Subscribe to orderbook updates with configurable depth (default: 100)
- Price precision interval: 0.001 (configurable)
- Supports snapshot and delta updates
- Returns: WsOrderBookEvent with bids/asks sorted appropriately

#### 6. watchOHLCV
- Method: `kline.subscribe`
- Subscribe to candlestick/OHLCV data
- Supported timeframes: 15m, 30m, 1h, 1d
- Returns: WsOhlcvEvent with timestamp, open, high, low, close, volume

## Message Format

### Subscribe Request
```json
{
  "method": "depth.subscribe",
  "params": ["BTC_USDT", 100, "0.001"],
  "id": 1706539608030
}
```

### Ticker Update (state.update)
```json
{
  "method": "state.update",
  "params": [
    "ETH_BTC",
    {
      "high": "0.055774",
      "close": "0.053679",
      "low": "0.053462",
      "last": "0.053679",
      "volume": "38463.6132",
      "open": "0.055682",
      "deal": "2091.0038055314"
    }
  ]
}
```

### Trades Update (deals.update)
```json
{
  "method": "deals.update",
  "params": [
    "ETH_BTC",
    [
      {
        "id": 4503032979,
        "amount": "0.103",
        "type": "sell",
        "time": 1657661950.8487639,
        "price": "0.05361"
      }
    ]
  ]
}
```

### OrderBook Update (depth.update)
```json
{
  "method": "depth.update",
  "params": [
    false,
    {
      "asks": [["19509.81", "0.277"]],
      "bids": [["19500.00", "1.5"]]
    },
    "BTC_USDT"
  ]
}
```

### OHLCV Update (kline.update)
```json
{
  "method": "kline.update",
  "params": [
    [
      1657648800,
      "0.054146",
      "0.053938",
      "0.054146",
      "0.053911",
      "596.4674",
      "32.2298758767",
      "ETH_BTC"
    ]
  ]
}
```

### Ping/Pong
Request:
```json
{
  "method": "server.ping",
  "params": [],
  "id": 1706539608030
}
```

Response:
```json
{
  "error": null,
  "result": "pong",
  "id": 1706539608030
}
```

## Implementation Details

### Symbol Format Conversion
- Unified format: `BTC/USDT`
- P2B format: `BTC_USDT`
- Conversion handled automatically

### Timeframe Mapping
- `15m` → 900 seconds
- `30m` → 1800 seconds
- `1h` → 3600 seconds
- `1d` → 86400 seconds

### OrderBook Delta Processing
- Snapshot: `params[0] = true` - full orderbook replacement
- Delta: `params[0] = false` - incremental update
- Delta logic:
  - Amount = 0 → remove price level
  - Existing price → update amount
  - New price → insert and re-sort

### Data Types
- Uses `rust_decimal::Decimal` for all price/amount values
- Timestamps in milliseconds (i64)
- Trade IDs as strings

## Testing

### Unit Tests
```rust
#[test]
fn test_p2b_ws_creation()
fn test_format_symbol()
fn test_parse_symbol()
fn test_timeframe_conversion()
```

## Integration with CCXT-Rust

### Module Registration
Added to `/Users/kevin/work/github/onlyhyde/trading/ccxt-rust/src/exchanges/foreign/mod.rs`:
- Module declaration: `mod p2b_ws;`
- Public export: `pub use p2b_ws::P2bWs;`

### Trait Implementation
Implements `WsExchange` trait with:
- Connection management
- Public market data streams
- Proper error handling
- Type-safe message passing via `mpsc::UnboundedReceiver<WsMessage>`

## Not Supported
- `watchOrderBookForSymbols` - P2B limitation
- `watchOhlcvForSymbols` - P2B limitation
- Private streams (balance, orders, my trades)
- Mark price / positions (futures-specific)

## Reference
Based on CCXT reference implementation:
- `/Users/kevin/work/github/onlyhyde/trading/ccxt-reference/js/src/pro/p2b.js`
- Official docs: https://github.com/P2B-team/P2B-WSS-Public/blob/main/wss_documentation.md

## Pattern Reference
Follows the same structure as:
- `/Users/kevin/work/github/onlyhyde/trading/ccxt-rust/src/exchanges/foreign/probit_ws.rs`

## Dependencies
- `async-trait`
- `futures-util`
- `serde_json`
- `tokio` with `sync`, `net` features
- `tokio-tungstenite`
- `rust_decimal`
- `chrono`
