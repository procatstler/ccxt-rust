# Performance Guide

Performance characteristics, benchmarks, and optimization tips for ccxt-rust.

## Overview

ccxt-rust is designed for high-performance trading applications with:
- Native Rust performance (no garbage collection)
- Zero-copy parsing where possible
- Efficient async I/O with Tokio
- Precise decimal arithmetic for financial calculations

## Running Benchmarks

### Prerequisites

```bash
# Install criterion for benchmarking
# Already included in dev-dependencies
```

### Run All Benchmarks

```bash
# Run all benchmarks
cargo bench

# Run specific benchmark group
cargo bench --bench exchange_benchmarks
cargo bench --bench parsing_benchmarks

# Run with specific feature
cargo bench --features cex
```

### Benchmark Output

Results are saved to `target/criterion/` with HTML reports.

```bash
# Open benchmark report
open target/criterion/report/index.html
```

## Benchmark Categories

### 1. Exchange Operations (`exchange_benchmarks.rs`)

Tests exchange instantiation and basic operations:

| Benchmark | Description | Typical Result |
|-----------|-------------|----------------|
| `binance_new` | Create Binance instance | ~1-2 μs |
| `bybit_new` | Create Bybit instance | ~1-2 μs |
| `okx_new` | Create OKX instance | ~1-2 μs |
| `exchange_id` | Get exchange ID | ~5-10 ns |
| `exchange_has` | Get feature map | ~50-100 ns |

### 2. Parsing Operations (`parsing_benchmarks.rs`)

Tests JSON parsing and data structure operations:

| Benchmark | Description | Typical Result |
|-----------|-------------|----------------|
| `decimal_multiply` | Decimal multiplication | ~10-20 ns |
| `decimal_divide` | Decimal division | ~50-100 ns |
| `decimal_from_str` | Parse string to Decimal | ~100-200 ns |
| `json_parse_ticker` | Parse ticker JSON | ~500-1000 ns |
| `json_parse_orderbook` | Parse small order book | ~1-2 μs |
| `json_parse_large_orderbook` | Parse 100-level order book | ~10-20 μs |
| `hmac_sha256` | Generate HMAC signature | ~500-800 ns |

### 3. Order Book Operations

| Benchmark | Description | Typical Result |
|-----------|-------------|----------------|
| `orderbook_clone_10` | Clone 10-level book | ~200-400 ns |
| `orderbook_clone_100` | Clone 100-level book | ~2-4 μs |
| `orderbook_best_bid` | Get best bid | ~5-10 ns |
| `orderbook_best_ask` | Get best ask | ~5-10 ns |

## Performance Comparison

### vs CCXT JavaScript

| Operation | CCXT JS | ccxt-rust | Improvement |
|-----------|---------|-----------|-------------|
| Exchange creation | ~1 ms | ~1-2 μs | ~500-1000x |
| JSON parsing | ~50 μs | ~1-2 μs | ~25-50x |
| Decimal arithmetic | ~1 μs | ~10-20 ns | ~50-100x |
| Memory per instance | ~10 MB | ~1 MB | ~10x |

*Note: Exact numbers depend on hardware and workload*

### vs CCXT Python

| Operation | CCXT Python | ccxt-rust | Improvement |
|-----------|-------------|-----------|-------------|
| Exchange creation | ~5 ms | ~1-2 μs | ~2500-5000x |
| JSON parsing | ~100 μs | ~1-2 μs | ~50-100x |
| Async overhead | High | Low | Significant |

## Optimization Tips

### 1. Reuse Exchange Instances

```rust
// Good: Create once, reuse
let exchange = Binance::new();
for symbol in symbols {
    let ticker = exchange.fetch_ticker(&symbol).await?;
}

// Bad: Create new instance each time
for symbol in symbols {
    let exchange = Binance::new();  // Unnecessary allocation
    let ticker = exchange.fetch_ticker(&symbol).await?;
}
```

### 2. Batch Requests When Possible

```rust
// Good: Fetch multiple tickers in one call
let tickers = exchange.fetch_tickers(&["BTC/USDT", "ETH/USDT"]).await?;

// Less efficient: Multiple individual calls
let btc = exchange.fetch_ticker("BTC/USDT").await?;
let eth = exchange.fetch_ticker("ETH/USDT").await?;
```

### 3. Use Streaming for Real-Time Data

```rust
// Good: WebSocket streaming (one connection, continuous data)
let mut rx = client.watch_ticker("BTC/USDT").await?;
while let Some(msg) = rx.recv().await {
    process(msg);
}

// Less efficient: Polling REST API
loop {
    let ticker = exchange.fetch_ticker("BTC/USDT").await?;
    process(ticker);
    tokio::time::sleep(Duration::from_secs(1)).await;
}
```

### 4. Parallel Execution

```rust
use futures::future::join_all;

// Good: Parallel requests
let futures: Vec<_> = symbols.iter()
    .map(|s| exchange.fetch_ticker(s))
    .collect();
let tickers = join_all(futures).await;

// Sequential (slower)
let mut tickers = Vec::new();
for symbol in &symbols {
    tickers.push(exchange.fetch_ticker(symbol).await?);
}
```

### 5. Efficient Order Book Processing

```rust
// Get best bid/ask directly
let best_bid = orderbook.bids.first();
let best_ask = orderbook.asks.first();

// Avoid unnecessary cloning
fn process_orderbook(ob: &OrderBook) {  // Borrow instead of clone
    // Process without copying
}
```

### 6. Connection Pooling

```rust
// HTTP client reuses connections by default
// No additional configuration needed

// For many concurrent requests, increase pool size
let config = ExchangeConfig {
    max_connections: 50,
    ..Default::default()
};
```

## Memory Optimization

### 1. Limit Order Book Depth

```rust
// Request only needed depth
let orderbook = exchange.fetch_order_book("BTC/USDT", Some(20)).await?;

// Don't request full depth if you only need top levels
// let orderbook = exchange.fetch_order_book("BTC/USDT", None).await?;  // May return 1000+ levels
```

### 2. Process Streaming Data Immediately

```rust
// Good: Process and discard
while let Some(msg) = rx.recv().await {
    if let WsMessage::Trade(event) = msg {
        save_to_database(&event).await;  // Process immediately
    }
    // event is dropped after each iteration
}

// Bad: Accumulate all data in memory
let mut all_trades = Vec::new();
while let Some(msg) = rx.recv().await {
    if let WsMessage::Trade(event) = msg {
        all_trades.push(event);  // Memory grows unbounded
    }
}
```

### 3. Use Appropriate Types

```rust
use rust_decimal::Decimal;

// Good: Use Decimal for financial precision
let price: Decimal = dec!(50000.12345678);

// Avoid: f64 for financial calculations (precision loss)
let price: f64 = 50000.12345678;
```

## Latency Considerations

### Network Latency

| Region | Typical Latency to Major Exchanges |
|--------|-----------------------------------|
| Same region | 1-5 ms |
| Cross-region | 50-200 ms |
| Cross-continent | 100-300 ms |

### Minimizing Latency

1. **Use regional endpoints**:
   ```rust
   let config = BinanceConfig {
       base_url: "https://api1.binance.com",  // Closest server
       ..Default::default()
   };
   ```

2. **Pre-warm connections**:
   ```rust
   // Make a lightweight call to establish connection
   exchange.fetch_time().await?;
   // Now subsequent calls reuse the connection
   ```

3. **Use WebSocket for time-sensitive operations**:
   ```rust
   // WebSocket: ~10-50ms latency
   // REST: ~100-200ms latency
   ```

## Profiling

### CPU Profiling

```bash
# Using perf (Linux)
cargo build --release
perf record ./target/release/your_binary
perf report

# Using Instruments (macOS)
cargo build --release
instruments -t "Time Profiler" ./target/release/your_binary
```

### Memory Profiling

```bash
# Using Valgrind
cargo build --release
valgrind --tool=massif ./target/release/your_binary
ms_print massif.out.*

# Using DHAT
cargo build --release --features dhat
./target/release/your_binary
```

### Async Profiling

```rust
// Enable tokio-console for async debugging
// In Cargo.toml:
// tokio = { version = "1", features = ["full", "tracing"] }
// console-subscriber = "0.1"

#[tokio::main]
async fn main() {
    console_subscriber::init();
    // Your code
}
```

## Benchmark Results Format

When running benchmarks, you'll see output like:

```
decimal_operations/decimal_multiply
                        time:   [12.345 ns 12.456 ns 12.567 ns]
                        change: [-1.23% +0.12% +1.45%] (p = 0.12 > 0.05)

json_parsing/json_parse_ticker
                        time:   [789.12 ns 800.00 ns 812.34 ns]
                        change: [-2.34% -1.23% -0.12%] (p = 0.01 < 0.05)
                        Performance has improved.
```

- **time**: [lower bound, estimate, upper bound]
- **change**: comparison to previous run
- **p value**: statistical significance

## Best Practices Summary

1. **Reuse** exchange instances and connections
2. **Batch** requests when possible
3. **Stream** instead of poll for real-time data
4. **Parallelize** independent requests
5. **Limit** data size (order book depth, history length)
6. **Profile** before optimizing
7. **Use** appropriate types (Decimal for money)
8. **Choose** correct region/endpoint

## Next Steps

- [Architecture Guide](./ARCHITECTURE.md)
- [WebSocket Guide](./WEBSOCKET_GUIDE.md)
- [Troubleshooting](./TROUBLESHOOTING.md)
