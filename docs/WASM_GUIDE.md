# WASM (WebAssembly) Guide

ccxt-rust can be compiled to WebAssembly for use in browsers and Node.js environments.

## Table of Contents

- [Building](#building)
- [Browser Usage](#browser-usage)
- [Supported Exchanges](#supported-exchanges)
- [REST API Examples](#rest-api-examples)
- [WebSocket Examples](#websocket-examples)
- [Utility Functions](#utility-functions)
- [Limitations](#limitations)
- [CORS Handling](#cors-handling)
- [Troubleshooting](#troubleshooting)

## Building

### Prerequisites

```bash
# Install wasm-pack
cargo install wasm-pack

# Install Rust WASM target
rustup target add wasm32-unknown-unknown
```

### Build Commands

```bash
# Build for web browsers
wasm-pack build --target web --no-default-features --features "wasm,cex"

# Build for Node.js
wasm-pack build --target nodejs --no-default-features --features "wasm,cex"

# Build for bundlers (webpack, etc.)
wasm-pack build --target bundler --no-default-features --features "wasm,cex"
```

### Output Structure

```
pkg/
├── ccxt_rust_bg.wasm    # ~700KB (optimized WebAssembly binary)
├── ccxt_rust.js         # JavaScript glue code
├── ccxt_rust.d.ts       # TypeScript type definitions
└── package.json         # npm package manifest
```

## Browser Usage

### Basic Setup

```html
<!DOCTYPE html>
<html>
<head>
    <title>ccxt-rust WASM Demo</title>
</head>
<body>
    <script type="module">
        import init, { WasmBinance, version } from './pkg/ccxt_rust.js';

        async function main() {
            // Initialize WASM module (required before any API calls)
            await init();

            console.log('ccxt-rust version:', version());

            // Create exchange instance
            const binance = new WasmBinance();

            // Fetch ticker
            const ticker = await binance.fetchTicker('BTC/USDT');
            console.log('BTC/USDT:', ticker.last);
        }

        main().catch(console.error);
    </script>
</body>
</html>
```

### With Configuration

```javascript
import init, { WasmBinance, WasmExchangeConfig } from './pkg/ccxt_rust.js';

await init();

// Create configuration
const config = new WasmExchangeConfig();
config.setTimeout(BigInt(30000));  // 30 second timeout
config.setSandbox(true);           // Use testnet

// Create exchange with config
const binance = new WasmBinance(config);
```

## Supported Exchanges

| Exchange | REST Class | WebSocket Class | Symbol Format |
|----------|------------|-----------------|---------------|
| Binance | `WasmBinance` | `WasmBinanceWs` | BTC/USDT |
| Upbit | `WasmUpbit` | `WasmUpbitWs` | BTC/KRW |
| Bybit | `WasmBybit` | `WasmBybitWs` | BTC/USDT |
| OKX | `WasmOkx` | - | BTC/USDT |
| Kraken | `WasmKraken` | - | BTC/USD |

### Available REST Methods

All exchanges support these methods:

| Method | Description | Parameters |
|--------|-------------|------------|
| `fetchTicker(symbol)` | Get single ticker | `symbol: string` |
| `fetchTickers(symbols?)` | Get multiple tickers | `symbols?: string[]` |
| `fetchOrderBook(symbol, limit?)` | Get order book | `symbol: string, limit?: number` |
| `fetchMarkets()` | Get all markets | - |
| `fetchTrades(symbol, limit?)` | Get recent trades | `symbol: string, limit?: number` |
| `fetchOhlcv(symbol, timeframe, since?, limit?)` | Get OHLCV candles | `symbol: string, timeframe: string, since?: number, limit?: number` |

### Timeframe Values

| Timeframe | Description |
|-----------|-------------|
| `1m` | 1 minute |
| `5m` | 5 minutes |
| `15m` | 15 minutes |
| `1h` | 1 hour |
| `4h` | 4 hours |
| `1d` | 1 day |
| `1w` | 1 week |

## REST API Examples

### Fetch Ticker

```javascript
const binance = new WasmBinance();

// Single ticker
const ticker = await binance.fetchTicker('BTC/USDT');
console.log('Symbol:', ticker.symbol);
console.log('Last:', ticker.last);
console.log('Bid:', ticker.bid);
console.log('Ask:', ticker.ask);
console.log('Volume:', ticker.volume);

// Get as JSON
const json = ticker.toJson();
console.log(JSON.parse(json));
```

### Fetch Multiple Tickers

```javascript
const binance = new WasmBinance();

// All tickers
const allTickers = await binance.fetchTickers();
console.log(JSON.parse(allTickers));

// Specific symbols
const selected = await binance.fetchTickers(['BTC/USDT', 'ETH/USDT']);
console.log(JSON.parse(selected));
```

### Fetch Order Book

```javascript
const binance = new WasmBinance();

const orderbook = await binance.fetchOrderBook('BTC/USDT', 10);
console.log('Symbol:', orderbook.symbol);
console.log('Bids:', orderbook.getBids());  // [[price, amount], ...]
console.log('Asks:', orderbook.getAsks());  // [[price, amount], ...]
console.log('Timestamp:', orderbook.timestamp);
```

### Fetch Markets

```javascript
const binance = new WasmBinance();

const markets = await binance.fetchMarkets();
const parsed = JSON.parse(markets);

parsed.forEach(market => {
    console.log(`${market.symbol}: ${market.base}/${market.quote}`);
});
```

### Fetch Trades

```javascript
const binance = new WasmBinance();

const trades = await binance.fetchTrades('BTC/USDT', 50);
const parsed = JSON.parse(trades);

parsed.forEach(trade => {
    console.log(`${trade.side} ${trade.amount} @ ${trade.price}`);
});
```

### Fetch OHLCV (Candles)

```javascript
const binance = new WasmBinance();

const candles = await binance.fetchOhlcv('BTC/USDT', '1h', null, 100);
const parsed = JSON.parse(candles);

parsed.forEach(candle => {
    console.log(`O:${candle[1]} H:${candle[2]} L:${candle[3]} C:${candle[4]} V:${candle[5]}`);
});
```

### Using Different Exchanges

```javascript
import init, {
    WasmBinance,
    WasmUpbit,
    WasmBybit,
    WasmOkx,
    WasmKraken
} from './pkg/ccxt_rust.js';

await init();

// Binance (global)
const binance = new WasmBinance();
const btcUsdt = await binance.fetchTicker('BTC/USDT');

// Upbit (Korean)
const upbit = new WasmUpbit();
const btcKrw = await upbit.fetchTicker('BTC/KRW');

// Bybit (derivatives)
const bybit = new WasmBybit();
const bybitTicker = await bybit.fetchTicker('BTC/USDT');

// OKX
const okx = new WasmOkx();
const okxTicker = await okx.fetchTicker('BTC/USDT');

// Kraken (US/EU)
const kraken = new WasmKraken();
const krakenTicker = await kraken.fetchTicker('BTC/USD');
```

## WebSocket Examples

### Watch Ticker

```javascript
import { WasmBinanceWs } from './pkg/ccxt_rust.js';

const ws = new WasmBinanceWs();

ws.watchTicker('BTC/USDT', (msg) => {
    switch (msg.message_type) {
        case 'connected':
            console.log('WebSocket connected');
            break;
        case 'ticker':
            const data = JSON.parse(msg.data);
            console.log('Price:', data.c);  // Last price
            break;
        case 'error':
            console.error('Error:', msg.data);
            break;
        case 'disconnected':
            console.log('WebSocket disconnected');
            break;
    }
});
```

### Watch Order Book

```javascript
import { WasmBybitWs } from './pkg/ccxt_rust.js';

const ws = new WasmBybitWs();

ws.watchOrderBook('BTC/USDT', 50, (msg) => {
    if (msg.message_type === 'orderbook') {
        const data = JSON.parse(msg.data);
        console.log('Bids:', data.bids.slice(0, 5));
        console.log('Asks:', data.asks.slice(0, 5));
    }
});
```

### Watch Trades

```javascript
import { WasmBinanceWs } from './pkg/ccxt_rust.js';

const ws = new WasmBinanceWs();

ws.watchTrades('BTC/USDT', (msg) => {
    if (msg.message_type === 'trade') {
        const trade = JSON.parse(msg.data);
        console.log(`${trade.side} ${trade.amount} @ ${trade.price}`);
    }
});
```

### WebSocket Message Types

```typescript
interface WasmWsMessage {
    message_type: 'connected' | 'disconnected' | 'ticker' | 'orderbook' | 'trade' | 'error';
    symbol: string;
    data: string;      // JSON string
    timestamp?: number;
}
```

## Utility Functions

```javascript
import {
    hmacSha256,           // HMAC-SHA256 signing
    hmacSha512,           // HMAC-SHA512 signing
    sha256,               // SHA256 hash
    sha512,               // SHA512 hash
    base64Encode,         // Base64 encode
    base64Decode,         // Base64 decode
    urlEncode,            // URL encode
    generateUuid,         // Generate UUID v4
    getCurrentTimestamp,  // Current timestamp (ms)
    getSupportedExchanges,// List supported exchanges
    version               // Library version
} from './pkg/ccxt_rust.js';

// Examples
const signature = hmacSha256('secret', 'message');
const hash = sha256('data');
const encoded = base64Encode('hello');
const decoded = base64Decode(encoded);
const uuid = generateUuid();
const timestamp = getCurrentTimestamp();  // Returns BigInt
const exchanges = JSON.parse(getSupportedExchanges());
const ver = version();
```

## Limitations

### Not Available in WASM

| Feature | Reason | Alternative |
|---------|--------|-------------|
| Private APIs (trading) | Security risk in browser | Use server-side proxy |
| File system access | Browser restriction | Use LocalStorage/IndexedDB |
| Environment variables | Browser restriction | Pass config from JavaScript |
| Tokio runtime | Native only | Uses wasm-bindgen-futures |
| `ring` crate | C dependencies | Uses pure Rust crypto |

### Security Considerations

- **Never expose API secrets in browser code**
- Use read-only API keys for public data
- Implement server-side proxy for trading operations
- Consider using browser extensions for development only

## CORS Handling

Most exchange APIs have CORS restrictions. Solutions:

### 1. Proxy Server (Recommended)

```javascript
// Configure proxy URL
const config = new WasmExchangeConfig();
// Use your proxy server that forwards to exchange API
```

### 2. Browser Extension (Development Only)

Use CORS-unblock extensions during development.

### 3. Exchange WebSocket

WebSocket connections typically don't have CORS restrictions:

```javascript
// WebSocket works without CORS issues
const ws = new WasmBinanceWs();
ws.watchTicker('BTC/USDT', callback);
```

### 4. Server-Side Rendering

For SSR frameworks (Next.js, Nuxt), use native Rust on server.

## Troubleshooting

### WASM Module Not Loading

```javascript
// Ensure init() is called before any API usage
await init();

// Check if WASM file is served with correct MIME type
// Server must return: application/wasm
```

### BigInt Errors

```javascript
// Some values return BigInt
const timestamp = getCurrentTimestamp();

// Convert to Number if needed (may lose precision for large values)
const timestampNum = Number(timestamp);

// Or use BigInt methods
config.setTimeout(BigInt(30000));
```

### Memory Leaks

```javascript
// Clean up WebSocket connections when done
// (Implementation specific - check if close() method available)
```

### Symbol Format Errors

```javascript
// Each exchange has specific symbol format
// Binance: 'BTC/USDT' (converts to 'BTCUSDT')
// Upbit: 'BTC/KRW' (converts to 'KRW-BTC')
// OKX: 'BTC/USDT' (converts to 'BTC-USDT')
// Kraken: 'BTC/USD' (converts to 'BTCUSD')
```

### Network Errors

```javascript
try {
    const ticker = await binance.fetchTicker('BTC/USDT');
} catch (error) {
    // Handle network or API errors
    console.error('API Error:', error);
}
```

## Package Size Optimization

Current package size: ~700KB (WASM) + ~74KB (JS)

To reduce size:
- Use `wasm-opt` (included in wasm-pack)
- Enable LTO in Cargo.toml
- Consider `wee_alloc` for smaller allocator

```toml
# Cargo.toml
[profile.release]
lto = true
opt-level = 's'  # or 'z' for smallest size
```

## CDN Usage

After npm publish:

```html
<!-- unpkg -->
<script type="module">
    import init from 'https://unpkg.com/@anthropic/ccxt-rust@latest/ccxt_rust.js';
</script>

<!-- jsdelivr -->
<script type="module">
    import init from 'https://cdn.jsdelivr.net/npm/@anthropic/ccxt-rust@latest/ccxt_rust.js';
</script>
```

## Related Documentation

- [Architecture Overview](./ARCHITECTURE.md)
- [WebSocket Guide](./WEBSOCKET_GUIDE.md)
- [Performance Tuning](./PERFORMANCE.md)
- [Troubleshooting](./TROUBLESHOOTING.md)
