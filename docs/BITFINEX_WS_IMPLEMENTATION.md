# Bitfinex WebSocket Implementation

## Overview
Implemented WebSocket support for Bitfinex exchange in `/Users/kevin/work/github/onlyhyde/trading/ccxt-rust/src/exchanges/foreign/bitfinex_ws.rs`.

## Features Implemented

### 1. Public Streams
- **watch_ticker()** - Subscribe to real-time ticker updates
- **watch_order_book()** - Subscribe to order book channel with configurable precision (P0-P3)
- **watch_trades()** - Subscribe to trades channel for real-time trade execution data
- **watch_ohlcv()** - Subscribe to candles channel for OHLCV data with multiple timeframes

### 2. WebSocket Configuration
- **URL**: `wss://api-pub.bitfinex.com/ws/2`
- **Auto-reconnect**: Enabled with 5-second interval
- **Max reconnect attempts**: 10
- **Ping interval**: 30 seconds
- **Connect timeout**: 30 seconds

### 3. Subscription Formats

#### Ticker
```json
{
  "event": "subscribe",
  "channel": "ticker",
  "symbol": "tBTCUSD"
}
```

#### Order Book
```json
{
  "event": "subscribe",
  "channel": "book",
  "symbol": "tBTCUSD",
  "prec": "P0",
  "len": "25"
}
```

#### Trades
```json
{
  "event": "subscribe",
  "channel": "trades",
  "symbol": "tBTCUSD"
}
```

#### Candles (OHLCV)
```json
{
  "event": "subscribe",
  "channel": "candles",
  "key": "trade:1m:tBTCUSD"
}
```

### 4. Symbol Format
- **Input**: `BTC/USD` (unified format)
- **Bitfinex**: `tBTCUSD` (prefix 't' for trading pairs)
- **Conversion**: Automatic conversion between formats

### 5. Supported Timeframes
- 1m, 5m, 15m, 30m (minutes)
- 1h, 6h, 12h (hours)
- 1D (daily)
- 1W (weekly)
- 1M (monthly)

## Architecture

### Key Components

1. **BitfinexWs struct**
   - Manages WebSocket client lifecycle
   - Tracks subscriptions and channel mappings
   - Handles event processing and message routing

2. **Channel Mapping**
   - Maps channel IDs to (channel_name, symbol) pairs
   - Enables proper message routing to subscribers

3. **Message Processing**
   - Handles subscription events (subscribed, info, error)
   - Processes data messages by channel type
   - Parses heartbeat messages
   - Converts Bitfinex format to unified types

4. **Decimal Parsing**
   - Helper function `parse_decimal()` handles both string and number JSON values
   - Supports Bitfinex's mixed-type responses

### Message Flow

```
User Request → subscribe_stream()
    ↓
WebSocket Connection → WS_BASE_URL
    ↓
Send Subscription → {"event": "subscribe", ...}
    ↓
Receive Events → process_message()
    ↓
Parse & Convert → Ticker/OrderBook/Trade/OHLCV
    ↓
Send to Channel → mpsc::UnboundedReceiver<WsMessage>
```

## Implementation Details

### 1. Ticker Parsing
- **Format**: `[BID, BID_SIZE, ASK, ASK_SIZE, DAILY_CHANGE, DAILY_CHANGE_RELATIVE, LAST_PRICE, VOLUME, HIGH, LOW]`
- **Mapping**: Converts array indices to Ticker struct fields
- **Timestamp**: Generated on receipt (Bitfinex doesn't include ticker timestamp)

### 2. Order Book Parsing
- **Snapshot**: Array of `[PRICE, COUNT, AMOUNT]` arrays
- **Update**: Single `[PRICE, COUNT, AMOUNT]` entry
- **Side determination**: Positive AMOUNT = bid, negative = ask
- **Count > 0**: Valid entry; Count = 0 means remove

### 3. Trade Parsing
- **Format**: `[ID, MTS, AMOUNT, PRICE]`
- **Snapshot**: Array of trade arrays
- **Update**: Single trade or ["te"/"tu", trade_data]
- **Side determination**: Positive AMOUNT = buy, negative = sell

### 4. Candle Parsing
- **Format**: `[MTS, OPEN, CLOSE, HIGH, LOW, VOLUME]`
- **Timeframe**: Extracted from subscription key
- **Real-time**: Updates as candles form

## Tests

All tests pass successfully:
```
test exchanges::foreign::bitfinex_ws::tests::test_format_symbol ... ok
test exchanges::foreign::bitfinex_ws::tests::test_format_timeframe ... ok
test exchanges::foreign::bitfinex_ws::tests::test_to_unified_symbol ... ok
```

### Test Coverage
1. Symbol format conversion (BTC/USD ↔ tBTCUSD)
2. Timeframe format conversion (Timeframe enum ↔ Bitfinex string)
3. Unified symbol parsing (tBTCUSD → BTC/USD)

## Usage Example

```rust
use ccxt_rust::exchanges::foreign::BitfinexWs;
use ccxt_rust::types::WsExchange;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ws = BitfinexWs::new();

    // Subscribe to BTC/USD ticker
    let mut ticker_rx = ws.watch_ticker("BTC/USD").await?;

    while let Some(msg) = ticker_rx.recv().await {
        match msg {
            WsMessage::Ticker(event) => {
                println!("Ticker: {:?}", event.ticker);
            }
            WsMessage::Connected => {
                println!("Connected to Bitfinex WebSocket");
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

## File Structure

```
src/exchanges/foreign/
├── bitfinex.rs         # REST API implementation (already exists)
├── bitfinex_ws.rs      # WebSocket implementation (NEW)
└── mod.rs              # Module exports (updated)
```

## Dependencies
- No new dependencies required
- Uses existing project dependencies:
  - `async-trait` for async trait implementation
  - `rust_decimal` for precise decimal handling
  - `tokio` for async runtime and channels
  - `serde_json` for JSON parsing

## Compilation Status
✅ **Build successful** - No errors or warnings for bitfinex_ws module
✅ **Tests passing** - All unit tests pass
✅ **Module exported** - Added to `mod.rs` exports

## References
- Bitfinex WebSocket API: https://docs.bitfinex.com/docs/ws-general
- Channel Documentation: https://docs.bitfinex.com/docs/ws-public
- Symbol Format: Trading pairs prefixed with 't' (e.g., tBTCUSD)

## Future Enhancements (Not Implemented)
- Private WebSocket streams (watch_balance, watch_orders, watch_my_trades)
- Authentication support
- Multiple ticker/trade/orderbook subscriptions in single connection
- Automatic channel ID management for unsubscribe operations
- Rate limiting for subscriptions
- Connection pooling for multiple symbols
