# Migration Guide: CCXT JavaScript to ccxt-rust

Guide for developers migrating from the original CCXT JavaScript/Python library to ccxt-rust.

## Overview

ccxt-rust is a Rust port of the popular CCXT library. While the API design is similar, there are important differences due to Rust's type system and async model.

## Key Differences

### 1. Type System

**JavaScript (Dynamic)**:
```javascript
const ticker = await exchange.fetchTicker('BTC/USDT');
console.log(ticker.last);  // Could be number, string, or undefined
```

**Rust (Static)**:
```rust
let ticker = exchange.fetch_ticker("BTC/USDT").await?;
println!("{:?}", ticker.last);  // Option<Decimal> - always defined type
```

### 2. Error Handling

**JavaScript**:
```javascript
try {
    const order = await exchange.createOrder('BTC/USDT', 'limit', 'buy', 1, 50000);
} catch (e) {
    if (e instanceof ccxt.InsufficientFunds) {
        console.log('Not enough funds');
    }
}
```

**Rust**:
```rust
match exchange.create_order("BTC/USDT", OrderType::Limit, OrderSide::Buy, amount, Some(price)).await {
    Ok(order) => println!("Order created: {:?}", order),
    Err(CcxtError::InsufficientFunds { message }) => {
        println!("Not enough funds: {}", message);
    }
    Err(e) => println!("Other error: {}", e),
}
```

### 3. Async/Await

**JavaScript**:
```javascript
// Promises or async/await
const ticker = await exchange.fetchTicker('BTC/USDT');

// Or with promises
exchange.fetchTicker('BTC/USDT').then(ticker => {
    console.log(ticker);
});
```

**Rust**:
```rust
// Always async/await with Tokio runtime
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ticker = exchange.fetch_ticker("BTC/USDT").await?;
    println!("{:?}", ticker);
    Ok(())
}
```

### 4. Configuration

**JavaScript**:
```javascript
const exchange = new ccxt.binance({
    apiKey: 'YOUR_API_KEY',
    secret: 'YOUR_SECRET',
    timeout: 30000,
    enableRateLimit: true,
});
```

**Rust**:
```rust
use ccxt_rust::exchanges::cex::Binance;

let exchange = Binance::new_with_credentials(
    "YOUR_API_KEY",
    "YOUR_SECRET",
);

// Or with full config
let config = BinanceConfig {
    api_key: Some("YOUR_API_KEY".to_string()),
    secret: Some("YOUR_SECRET".to_string()),
    timeout: Duration::from_secs(30),
    ..Default::default()
};
let exchange = Binance::new_with_config(config);
```

## API Mapping

### Exchange Initialization

| JavaScript | Rust |
|------------|------|
| `new ccxt.binance()` | `Binance::new()` |
| `new ccxt.binance({ apiKey, secret })` | `Binance::new_with_credentials(key, secret)` |
| `exchange.setSandboxMode(true)` | `Binance::new_sandbox()` |

### Public API Methods

| JavaScript | Rust |
|------------|------|
| `fetchMarkets()` | `fetch_markets().await` |
| `fetchTicker(symbol)` | `fetch_ticker(symbol).await` |
| `fetchTickers(symbols)` | `fetch_tickers(symbols).await` |
| `fetchOrderBook(symbol, limit)` | `fetch_order_book(symbol, limit).await` |
| `fetchTrades(symbol, since, limit)` | `fetch_trades(symbol, since, limit).await` |
| `fetchOHLCV(symbol, timeframe, since, limit)` | `fetch_ohlcv(symbol, timeframe, since, limit).await` |

### Private API Methods

| JavaScript | Rust |
|------------|------|
| `fetchBalance()` | `fetch_balance().await` |
| `createOrder(symbol, type, side, amount, price)` | `create_order(symbol, type, side, amount, price).await` |
| `cancelOrder(id, symbol)` | `cancel_order(id, symbol).await` |
| `fetchOrder(id, symbol)` | `fetch_order(id, symbol).await` |
| `fetchOrders(symbol, since, limit)` | `fetch_orders(symbol, since, limit).await` |
| `fetchOpenOrders(symbol)` | `fetch_open_orders(symbol).await` |
| `fetchClosedOrders(symbol)` | `fetch_closed_orders(symbol).await` |

### WebSocket Methods

| JavaScript | Rust |
|------------|------|
| `watchTicker(symbol)` | `watch_ticker(symbol).await` |
| `watchOrderBook(symbol, limit)` | `watch_order_book(symbol, limit).await` |
| `watchTrades(symbol)` | `watch_trades(symbol).await` |
| `watchOHLCV(symbol, timeframe)` | `watch_ohlcv(symbol, timeframe).await` |
| `watchBalance()` | `watch_balance().await` |
| `watchOrders(symbol)` | `watch_orders(symbol).await` |

## Type Mapping

### Primitives

| JavaScript | Rust |
|------------|------|
| `number` | `Decimal` (for prices/amounts) or `f64` |
| `string` | `String` or `&str` |
| `boolean` | `bool` |
| `undefined/null` | `Option<T>` |
| `object` | Struct or `HashMap` |
| `array` | `Vec<T>` |

### CCXT Types

| JavaScript | Rust |
|------------|------|
| `Market` | `Market` |
| `Ticker` | `Ticker` |
| `OrderBook` | `OrderBook` |
| `Trade` | `Trade` |
| `Order` | `Order` |
| `Balance` | `Balance` |
| `OHLCV` (array) | `Ohlcv` (struct) |

### Enums

| JavaScript | Rust |
|------------|------|
| `'buy'` / `'sell'` | `OrderSide::Buy` / `OrderSide::Sell` |
| `'market'` / `'limit'` | `OrderType::Market` / `OrderType::Limit` |
| `'open'` / `'closed'` / `'canceled'` | `OrderStatus::Open` / `OrderStatus::Closed` / `OrderStatus::Canceled` |
| `'1m'` / `'1h'` / `'1d'` | `Timeframe::Minute1` / `Timeframe::Hour1` / `Timeframe::Day1` |

## Code Migration Examples

### Example 1: Fetching Ticker

**JavaScript**:
```javascript
const ccxt = require('ccxt');

async function main() {
    const exchange = new ccxt.binance();
    const ticker = await exchange.fetchTicker('BTC/USDT');
    console.log(`Last price: ${ticker.last}`);
    console.log(`24h volume: ${ticker.quoteVolume}`);
}

main();
```

**Rust**:
```rust
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::types::Exchange;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let exchange = Binance::new();
    let ticker = exchange.fetch_ticker("BTC/USDT").await?;

    println!("Last price: {:?}", ticker.last);
    println!("24h volume: {:?}", ticker.quote_volume);

    Ok(())
}
```

### Example 2: Creating an Order

**JavaScript**:
```javascript
const ccxt = require('ccxt');

async function main() {
    const exchange = new ccxt.binance({
        apiKey: 'YOUR_API_KEY',
        secret: 'YOUR_SECRET',
    });

    try {
        const order = await exchange.createOrder(
            'BTC/USDT',
            'limit',
            'buy',
            0.001,
            50000
        );
        console.log(`Order ID: ${order.id}`);
    } catch (e) {
        console.error(`Error: ${e.message}`);
    }
}

main();
```

**Rust**:
```rust
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::types::{Exchange, OrderType, OrderSide};
use rust_decimal_macros::dec;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let exchange = Binance::new_with_credentials(
        "YOUR_API_KEY",
        "YOUR_SECRET",
    );

    let order = exchange.create_order(
        "BTC/USDT",
        OrderType::Limit,
        OrderSide::Buy,
        dec!(0.001),
        Some(dec!(50000)),
    ).await?;

    println!("Order ID: {}", order.id);

    Ok(())
}
```

### Example 3: WebSocket Streaming

**JavaScript**:
```javascript
const ccxt = require('ccxt');

async function main() {
    const exchange = new ccxt.pro.binance();

    while (true) {
        const ticker = await exchange.watchTicker('BTC/USDT');
        console.log(`${ticker.symbol}: ${ticker.last}`);
    }
}

main();
```

**Rust**:
```rust
use ccxt_rust::exchanges::cex::BinanceWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = BinanceWs::new();
    let mut rx = client.watch_ticker("BTC/USDT").await?;

    while let Some(msg) = rx.recv().await {
        if let WsMessage::Ticker(event) = msg {
            println!("{}: {:?}", event.ticker.symbol, event.ticker.last);
        }
    }

    Ok(())
}
```

### Example 4: Error Handling

**JavaScript**:
```javascript
const ccxt = require('ccxt');

async function main() {
    const exchange = new ccxt.binance({ apiKey: 'key', secret: 'secret' });

    try {
        await exchange.fetchBalance();
    } catch (e) {
        if (e instanceof ccxt.AuthenticationError) {
            console.log('Invalid credentials');
        } else if (e instanceof ccxt.NetworkError) {
            console.log('Network issue');
        } else if (e instanceof ccxt.ExchangeError) {
            console.log('Exchange error:', e.message);
        }
    }
}
```

**Rust**:
```rust
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::types::{Exchange, CcxtError};

#[tokio::main]
async fn main() {
    let exchange = Binance::new_with_credentials("key", "secret");

    match exchange.fetch_balance().await {
        Ok(balance) => println!("{:?}", balance),
        Err(CcxtError::AuthenticationError { message }) => {
            println!("Invalid credentials: {}", message);
        }
        Err(CcxtError::NetworkError { message }) => {
            println!("Network issue: {}", message);
        }
        Err(CcxtError::ExchangeError { message }) => {
            println!("Exchange error: {}", message);
        }
        Err(e) => println!("Other error: {}", e),
    }
}
```

## Feature Comparison

| Feature | CCXT JS | ccxt-rust |
|---------|---------|-----------|
| Exchanges | 100+ | 100+ CEX, 8 DEX |
| REST API | Yes | Yes |
| WebSocket | Yes (ccxt.pro) | Yes (included) |
| Type Safety | No | Yes |
| Async/Await | Yes | Yes (Tokio) |
| Rate Limiting | Built-in | Manual |
| Proxy Support | Yes | Via reqwest config |
| Browser Support | Yes | No (server-side only) |

## Common Pitfalls

### 1. Forgetting `.await`

```rust
// Wrong - returns Future, not result
let ticker = exchange.fetch_ticker("BTC/USDT");

// Correct
let ticker = exchange.fetch_ticker("BTC/USDT").await?;
```

### 2. Not Handling Options

```rust
// Wrong - might panic
let price = ticker.last.unwrap();

// Correct
if let Some(price) = ticker.last {
    println!("Price: {}", price);
}

// Or with default
let price = ticker.last.unwrap_or_default();
```

### 3. String vs &str

```rust
// Function takes &str
exchange.fetch_ticker("BTC/USDT").await?;

// If you have String, use &
let symbol = String::from("BTC/USDT");
exchange.fetch_ticker(&symbol).await?;
```

### 4. Decimal Precision

```rust
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

// Use Decimal for financial calculations
let amount = dec!(0.001);
let price = dec!(50000);
let cost = amount * price;  // Precise calculation
```

## Performance Benefits

Moving to Rust provides:

1. **Memory Safety**: No null pointer exceptions or memory leaks
2. **Concurrency**: Safe parallel execution with async/await
3. **Speed**: Native performance, no garbage collection pauses
4. **Type Safety**: Catch errors at compile time
5. **Lower Latency**: Important for trading applications

## Next Steps

1. [Quick Start Guide](../README.md)
2. [WebSocket Guide](./WEBSOCKET_GUIDE.md)
3. [API Reference](https://docs.rs/ccxt-rust)
4. [Examples](../examples/)
