# IndependentReserve WebSocket Implementation

## Overview

Implementation of IndependentReserve WebSocket API for ccxt-rust, providing real-time market data streaming for trades and order books.

## Files Created

- `/src/exchanges/foreign/independentreserve_ws.rs` - Main WebSocket implementation
- `/examples/independentreserve_ws_example.rs` - Usage example
- Updated `/src/exchanges/foreign/mod.rs` - Module exports

## Implementation Details

### WebSocket URL Structure

IndependentReserve uses different WebSocket URLs for different channels:

1. **Trades**: `wss://websockets.independentreserve.com?subscribe=ticker-{base}-{quote}`
   - Example: `wss://websockets.independentreserve.com?subscribe=ticker-btc-aud`

2. **OrderBook**: `wss://websockets.independentreserve.com/orderbook/{limit}?subscribe={base}-{quote}`
   - Example: `wss://websockets.independentreserve.com/orderbook/100?subscribe=eth-aud`

### Message Format

#### Trade Events
```json
{
  "Channel": "ticker-btc-usd",
  "Nonce": 130,
  "Data": {
    "TradeGuid": "7a669f2a-d564-472b-8493-6ef982eb1e96",
    "Pair": "btc-aud",
    "TradeDate": "2023-02-12T10:04:13.0804889+11:00",
    "Price": 31640,
    "Volume": 0.00079029,
    "Side": "Buy"
  },
  "Time": 1676156653111,
  "Event": "Trade"
}
```

#### OrderBook Events

**Snapshot**:
```json
{
  "Channel": "orderbook/1/eth/aud",
  "Data": {
    "Bids": [
      { "Price": 2198.09, "Volume": 0.16143952 }
    ],
    "Offers": [
      { "Price": 2201.25, "Volume": 15 }
    ],
    "Crc32": 1519697650
  },
  "Time": 1676150558254,
  "Event": "OrderBookSnapshot"
}
```

**Delta**:
```json
{
  "Channel": "orderbook/1/eth/aud",
  "Data": {
    "Bids": [
      { "Price": 2198.09, "Volume": 0.16143952 }
    ],
    "Offers": [
      { "Price": 2201.25, "Volume": 15 }
    ]
  },
  "Time": 1676150558254,
  "Event": "OrderBookChange"
}
```

#### Heartbeat
```json
{
  "Time": 1676156208182,
  "Event": "Heartbeat"
}
```

### Architecture

#### Connection Management

IndependentReserve requires separate WebSocket connections for different channels. The implementation creates a new connection for each subscription:

- Each `watch_trades()` call creates a dedicated connection
- Each `watch_order_book()` call creates a dedicated connection

This differs from exchanges like ProBit where a single connection handles multiple subscriptions.

#### Message Handling

The implementation follows the established pattern:

1. **Message Loop**: Each connection spawns a tokio task that processes incoming messages
2. **Event Routing**: Messages are routed based on the "Event" field
3. **Channel Distribution**: Processed data is sent to subscribers via mpsc channels

### Data Parsing

#### Trade Parsing
- **ID**: `TradeGuid` field
- **Price/Volume**: Parsed as `f64` and converted to `Decimal`
- **Timestamp**: ISO 8601 datetime parsed with `chrono`
- **Side**: Converted to lowercase ("buy"/"sell")

#### OrderBook Parsing
- **Channel Parsing**: Extract symbol and limit from channel string
- **Bids/Asks**: Parse from "Bids" and "Offers" arrays
- **Snapshot vs Delta**: Determined by "Event" field
- **Delta Application**: Merge updates with cached orderbook

### Supported Features

✅ Implemented:
- `watch_trades()` - Real-time trade stream
- `watch_order_book()` - Real-time orderbook with configurable depth

❌ Not Supported (per exchange limitations):
- `watch_ticker()` - Exchange doesn't provide ticker stream
- `watch_tickers()` - Exchange doesn't provide ticker stream
- `watch_ohlcv()` - Exchange doesn't provide OHLCV stream
- `watch_trades_for_symbols()` - Would require multiple connections
- `watch_order_book_for_symbols()` - Would require multiple connections

### Implementation Patterns

Following the established patterns from `probit_ws.rs`:

1. **Dependencies**:
   ```rust
   use tokio_tungstenite::{connect_async, ...};
   use rust_decimal::Decimal;
   use async_trait::async_trait;
   ```

2. **State Management**:
   ```rust
   pub struct IndependentReserveWs {
       ws_stream: Option<Arc<RwLock<WebSocketStream<...>>>>,
       subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
       orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
   }
   ```

3. **Trade Builder Pattern**:
   ```rust
   let trade = Trade::new(id, symbol, price, amount)
       .with_timestamp(ts)
       .with_side(&side);
   ```

4. **OrderBook Delta Updates**:
   ```rust
   fn apply_deltas(book_side: &mut Vec<OrderBookEntry>, updates: &Vec<OrderBookEntry>, is_bid: bool)
   ```

### Testing

Unit tests included:
- `test_independentreserve_ws_creation()` - Basic instantiation
- `test_parse_symbol_parts()` - Symbol parsing
- `test_market_id_to_symbol()` - Market ID conversion

### Known Limitations

1. **Multiple Connections**: Each subscription requires a separate WebSocket connection
2. **Connection Storage**: Current implementation overwrites `ws_stream` - production use would need better multi-connection management
3. **Checksum Validation**: Not implemented (reference implementation has TODO for checksum)
4. **Reconnection**: No automatic reconnection on disconnect

### Future Improvements

1. **Connection Pool**: Manage multiple WebSocket connections efficiently
2. **Checksum Validation**: Implement CRC32 checksum validation for orderbooks
3. **Auto-Reconnection**: Add reconnection logic with exponential backoff
4. **Rate Limiting**: Handle subscription rate limits
5. **Connection Reuse**: Investigate if multiple subscriptions can share connections

## Usage Example

```rust
use ccxt_rust::exchanges::foreign::IndependentReserveWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ws = IndependentReserveWs::new();
    ws.ws_connect().await?;

    // Subscribe to trades
    let mut trades_rx = ws.watch_trades("BTC/AUD").await?;

    // Subscribe to orderbook
    let mut orderbook_rx = ws.watch_order_book("ETH/AUD", Some(10)).await?;

    // Process messages
    while let Some(msg) = trades_rx.recv().await {
        if let WsMessage::Trade(event) = msg {
            for trade in &event.trades {
                println!("Trade: {} @ {}", trade.amount, trade.price);
            }
        }
    }

    ws.ws_close().await?;
    Ok(())
}
```

## Comparison with Reference Implementation

### Similarities
- URL structure matches reference implementation
- Message parsing follows the same logic
- Event handling structure is identical

### Differences
- **Connection Management**: Rust implementation creates separate tasks per connection
- **Error Handling**: Uses Result types instead of exceptions
- **Type Safety**: Strongly typed with Decimal for prices/amounts
- **Async Runtime**: Uses tokio instead of JavaScript promises

## Integration Notes

The implementation integrates seamlessly with the existing ccxt-rust architecture:

1. Follows the `WsExchange` trait
2. Uses standard message types (`WsTradeEvent`, `WsOrderBookEvent`)
3. Matches error handling patterns
4. Compatible with existing example patterns

## Testing Recommendations

1. **Live Testing**: Test against actual IndependentReserve WebSocket
2. **Symbol Variations**: Test with different currency pairs
3. **Orderbook Depths**: Test with various limit values (1, 10, 100)
4. **Long Running**: Verify stability over extended periods
5. **Reconnection**: Test behavior on connection drops

## Performance Considerations

1. **Memory**: Each connection maintains its own orderbook cache
2. **CPU**: JSON parsing and Decimal conversions per message
3. **Network**: Separate TCP connections for each subscription
4. **Threading**: Each connection runs in its own tokio task

## Maintenance Notes

- Monitor for API changes from IndependentReserve
- Update message format parsing if exchange modifies structure
- Consider implementing checksum validation when production-ready
- Review connection management strategy for high-volume scenarios
