# CCXT-Rust Porting Status

> Last Updated: 2026-01-11 (Directory restructuring to CEX/DEX, comprehensive WebSocket coverage)

## Overview

This document tracks the progress of porting the CCXT library from TypeScript/JavaScript to Rust.

### Summary Statistics

| Category | Implemented | Total | Progress |
|----------|-------------|-------|----------|
| CEX REST API | 101 | ~105 | 96% |
| CEX WebSocket | 100 | ~105 | 95% |
| DEX REST API | 8 | 8 | 100% |
| DEX WebSocket | 8 | 8 | 100% |
| **REST API Total** | **109** | **~113** | **96%** |
| **WebSocket Total** | **108** | **~113** | **96%** |
| **Private Channels** | **34+** | - | Comprehensive ✅ |

---

## Phase Status

| Phase | Description | Status |
|-------|-------------|--------|
| Phase 1 | High Priority WebSocket (Deribit, Hyperliquid, BitMEX, Bitstamp) | ✅ Complete |
| Phase 2 | Coinbase Family (Advanced, Exchange, International) | ✅ Complete |
| Phase 3 | Medium Priority REST (BitMart, Blockchain.com, CEX.io, Blofin) | ✅ Complete |
| Phase 4 | Regional Exchanges | ✅ Complete |
| Phase 5 | Directory Restructuring (CEX/DEX) | ✅ Complete |

---

## Implemented Exchanges

### Foreign Exchanges (57 REST + 49 WebSocket)

| # | Exchange | REST | WebSocket | Notes |
|---|----------|:----:|:---------:|-------|
| 1 | Alpaca | ✅ | ✅ | Stock/Crypto hybrid |
| 2 | Ascendex | ✅ | ✅ | |
| 3 | Binance | ✅ | ✅ | Main spot exchange |
| 4 | Binance Futures | ✅ | ✅ | USDM futures |
| 5 | Binance CoinM | ✅ | - | Coin-margined futures |
| 6 | Binance US | ✅ | - | |
| 7 | BingX | ✅ | ✅ | |
| 8 | Bitbank | ✅ | - | Japan |
| 9 | Bitfinex | ✅ | ✅ | |
| 10 | Bitflyer | ✅ | - | Japan |
| 11 | Bitget | ✅ | ✅ | |
| 12 | BitMart | ✅ | ✅ | |
| 13 | BitMEX | ✅ | ✅ | Derivatives |
| 14 | Bitso | ✅ | - | Mexico |
| 15 | Bitstamp | ✅ | ✅ | Major fiat gateway |
| 16 | Bitvavo | ✅ | ✅ | Netherlands |
| 17 | Blockchain.com | ✅ | - | Multi-chain |
| 18 | Blofin | ✅ | ✅ | Options platform |
| 19 | Bullish | ✅ | ✅ | Institutional |
| 20 | Backpack | ✅ | ✅ | Solana-based |
| 21 | BTCBox | ✅ | - | Japan |
| 20 | BTCMarkets | ✅ | - | Australia |
| 21 | Bybit | ✅ | ✅ | |
| 22 | CEX.io | ✅ | ✅ | Fiat gateway |
| 23 | Coinbase | ✅ | ✅ | |
| 24 | Coinbase Advanced | ✅ | - | Alias |
| 25 | Coinbase Exchange | ✅ | - | Separate API |
| 26 | Coinbase International | ✅ | - | International |
| 27 | Coincheck | ✅ | - | Japan |
| 28 | CoinEx | ✅ | ✅ | |
| 29 | Crypto.com | ✅ | ✅ | |
| 30 | Delta | ✅ | ✅ | India derivatives, Private channels |
| 31 | Deribit | ✅ | ✅ | Options/Futures |
| 32 | dYdX | ✅ | ✅ | DEX |
| 33 | Exmo | ✅ | ✅ | Europe |
| 34 | Foxbit | ✅ | - | Brazil |
| 34 | Gate.io | ✅ | ✅ | |
| 35 | Gemini | ✅ | ✅ | |
| 36 | HitBTC | ✅ | ✅ | |
| 37 | HTX (Huobi) | ✅ | ✅ | |
| 38 | Hyperliquid | ✅ | ✅ | DEX |
| 39 | Hashkey | ✅ | ✅ | Hong Kong |
| 40 | HollaEx | ✅ | ✅ | White-label, Private channels |
| 41 | Indodax | ✅ | - | Indonesia |
| 40 | Kraken | ✅ | ✅ | |
| 41 | Kraken Futures | ✅ | ✅ | |
| 42 | KuCoin | ✅ | ✅ | |
| 43 | KuCoin Futures | ✅ | ✅ | |
| 44 | Latoken | ✅ | - | |
| 45 | LBank | ✅ | ✅ | Private channels (MD5) |
| 46 | Mercado | ✅ | - | Brazil |
| 47 | MEXC | ✅ | ✅ | |
| 48 | OKX | ✅ | ✅ | |
| 49 | OneTrading | ✅ | ✅ | Europe |
| 50 | Phemex | ✅ | ✅ | |
| 50 | Poloniex | ✅ | ✅ | |
| 51 | Probit | ✅ | ✅ | Private channels (OAuth2) |
| 52 | Tokocrypto | ✅ | - | Indonesia |
| 53 | WhiteBit | ✅ | ✅ | |
| 54 | Woo | ✅ | ✅ | |
| 55 | Zaif | ✅ | - | Japan |
| 56 | Zebpay | ✅ | - | India |
| 57 | BitoPro | ✅ | - | Taiwan |
| 58 | CoinsPH | ✅ | - | Philippines |
| 59 | DeepCoin | ✅ | - | Derivatives |
| 60 | BitTrade | ✅ | - | Japan (HTX-based) |
| 61 | Apex | ✅ | - | DEX (StarkEx L2) |
| 62 | OXFun | ✅ | - | Derivatives |
| 63 | Defx | ✅ | - | DEX derivatives |
| 64 | Derive | ✅ | - | DEX (Lyra Finance) |
| 65 | (Various) | ✅ | Mixed | See file list |

### Korean Exchanges (4 REST + 4 WebSocket)

| # | Exchange | REST | WebSocket | Notes |
|---|----------|:----:|:---------:|-------|
| 1 | Upbit | ✅ | ✅ | Korea #1 |
| 2 | Bithumb | ✅ | ✅ | Korea #2 |
| 3 | Coinone | ✅ | ✅ | Korea #3 |
| 4 | Korbit | ✅ | ✅ | Korea #4 |

---

## WebSocket Private Channels Status

Private channels allow real-time streaming of user-specific data (orders, trades, balances).

### Major Exchanges - Full Implementation (22 exchanges)

| Exchange | Signing Method | watch_orders | watch_my_trades | watch_balance |
|----------|---------------|:------------:|:---------------:|:-------------:|
| Binance | Listen Key | ✅ | ✅ | ✅ |
| Bybit | HMAC-SHA256 | ✅ | ✅ | ✅ |
| OKX | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Kraken | HMAC-SHA512 | ✅ | ✅ | ✅ |
| KuCoin | HMAC-SHA256 | ✅ | ✅ | ✅ |
| MEXC | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Gate.io | HMAC-SHA512 | ✅ | ✅ | ✅ |
| HTX | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Bitget | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Bitfinex | HMAC-SHA384 | ✅ | ✅ | ✅ |
| Bitstamp | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Coinbase | JWT/HMAC | ✅ | ✅ | ✅ |
| Deribit | HMAC-SHA256 | ✅ | ✅ | ✅ |
| BitMEX | HMAC-SHA256 | ✅ | ✅ | ✅ |
| BingX | HMAC-SHA256 | ✅ | ✅ | ✅ |
| BitMart | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Crypto.com | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Ascendex | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Woo | HMAC-SHA256 | ✅ | ✅ | - |
| Blofin | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Phemex | HMAC-SHA256 | ✅ | ✅ | ✅ |
| Poloniex | HMAC-SHA256 | ✅ | ✅ | ✅ |

### Priority 1 Exchanges - Custom Implementation (12 exchanges)

| Exchange | Signing Method | watch_orders | watch_my_trades | watch_balance | Tests |
|----------|---------------|:------------:|:---------------:|:-------------:|:-----:|
| Upbit | HMAC-SHA256 | ✅ | ✅ | ✅ | ✅ |
| Bithumb | HMAC-SHA256 | ✅ | ✅ | ✅ | ✅ |
| HitBTC | HMAC-SHA256 | ✅ | ✅ | ✅ | ✅ |
| Bitrue | HMAC-SHA256 | ✅ | ✅ | ✅ | ✅ |
| CoinEx | HMAC-SHA256 | ✅ | ✅ | ✅ | ✅ |
| HashKey | HMAC-SHA256 | ✅ | ✅ | ✅ | ✅ |
| LBank | MD5 | ✅ | ✅ | ✅ | 11 |
| ProBit | OAuth2 Token | ✅ | ✅ | ✅ | 8 |
| WhiteBIT | HMAC-SHA512 | ✅ | ✅ | ✅ | 9 |
| XT | HMAC-SHA256 | ✅ | ✅ | ✅ | 10 |
| Delta | HMAC-SHA256 | ✅ | ✅ | ✅ | 12 |
| HollaEx | HMAC-SHA256 | ✅ | - | ✅ | 5 |

### Alternative Authentication Methods

| Exchange | Auth Method | Notes |
|----------|-------------|-------|
| dYdX | Subaccount ID | DEX with account-based auth |
| Hyperliquid | Wallet Address | DEX with wallet-based auth |
| Paradex | JWT Token | StarkNet-based DEX |

### Alias Exchanges (Inherit from Parent)

| Alias | Parent | Notes |
|-------|--------|-------|
| BinanceUS, BinanceUSDM, BinanceCoinM | Binance | Full private channel support |
| MyOKX, OkxUS | OKX | Full private channel support |
| GateIO | Gate.io | Full private channel support |
| Huobi | HTX | Full private channel support |
| CoinbaseAdvanced | Coinbase | Full private channel support |

### Not Supported

| Exchange | Reason |
|----------|--------|
| Gemini | API doesn't support watchOrders |
| LATOKEN | WebSocket authentication not available |

### Implementation Pattern

Each private channel implementation includes:
- `sign()`: Creates authentication signature
- `parse_order()`: Converts exchange order format to unified `Order` type
- `parse_balance()`: Converts exchange balance format to unified `Balances` type
- `ws_authenticate()`: Authenticates WebSocket connection
- Private data types: `*OrderUpdateData`, `*BalanceUpdateData`

---

## Unimplemented Exchanges (Phase 4)

### Remaining (~6 exchanges)

| Exchange | Reference Lines | Region | Notes |
|----------|----------------|--------|-------|
| Arkham | ~900 | Global | Intelligence platform |
| BitTeam | ~800 | Russia | |
| CoincCatch | ~900 | Global | |
| Hibachi | ~800 | Global | |
| ModeTrace | ~800 | Global | |
| WooFi Pro | ~800 | Global | |

---

## File Structure

```
src/exchanges/
├── cex/               # Centralized Exchanges (201 files)
│   ├── mod.rs
│   ├── binance.rs
│   ├── binance_ws.rs
│   ├── bybit.rs
│   ├── bybit_ws.rs
│   ├── okx.rs
│   ├── okx_ws.rs
│   ├── upbit.rs       # Korean exchanges included
│   ├── upbit_ws.rs
│   ├── bithumb.rs
│   ├── bithumb_ws.rs
│   └── ... (190 more files)
├── dex/               # Decentralized Exchanges (17 files)
│   ├── mod.rs
│   ├── hyperliquid.rs
│   ├── hyperliquid_ws.rs
│   ├── dydx.rs
│   ├── dydx_ws.rs
│   ├── dydxv4.rs
│   ├── dydxv4_ws.rs
│   ├── paradex.rs
│   ├── paradex_ws.rs
│   └── ... (8 more files)
└── mod.rs             # Module exports (CEX + DEX re-exports)
```

---

## Test Coverage

- **Total Tests**: 1,558+
- **Unit Tests**: 1,543 passed
- **Integration Tests**: 15 passed
- **Doc Tests**: 2 passed (16 ignored - require network)
- **Clippy Warnings**: 9 (acceptable - too_many_arguments)

Run tests:
```bash
cargo test              # All tests
cargo test --lib        # Unit tests only
cargo clippy            # Lint check
cargo doc               # Build documentation
```

---

## Contributing

When implementing a new exchange:

1. Reference `ccxt-reference/ts/src/{exchange}.ts` for REST
2. Reference `ccxt-reference/js/src/pro/{exchange}.js` for WebSocket
3. Determine if CEX or DEX:
   - **CEX**: Add to `src/exchanges/cex/` (follow `binance.rs` pattern)
   - **DEX**: Add to `src/exchanges/dex/` (follow `hyperliquid.rs` pattern)
4. Implement `Exchange` trait for REST
5. Implement `WsExchange` trait for WebSocket
6. Add to appropriate `mod.rs` exports (cex/mod.rs or dex/mod.rs)
7. Add re-export to `src/exchanges/mod.rs`
8. Add to `ExchangeId` enum in `src/types/exchange.rs`

---

## Changelog

### 2026-01-11 (Directory Restructuring & Project Setup)
- **Directory Restructuring**: Reorganized from `foreign/korean` to `cex/dex` structure
  - CEX: 201 files (101 REST + 100 WebSocket)
  - DEX: 17 files (8 REST + 8 WebSocket + 1 order module)
- **Project Setup**:
  - Added README.md with comprehensive documentation
  - Added LICENSE (MIT)
  - Added CI/CD with GitHub Actions (build, test, clippy, fmt, doc)
  - Added Dependabot configuration
  - Updated Cargo.toml with full metadata
- **Code Quality**:
  - Fixed 21 doc warnings (URL hyperlinks)
  - All tests passing (1,558+)
  - Clippy clean (9 acceptable warnings)

### 2026-01-04 (WebSocket Private Channels - Comprehensive Analysis)
- **Comprehensive Private Channel Analysis** completed:
  - 22 major exchanges already have full private channel implementation
  - 10 Priority 1 exchanges with custom implementation
  - 5+ alias exchanges inherit from parent
  - 3 DEX exchanges use alternative auth (wallet/subaccount)
  - 2 exchanges (Gemini, LATOKEN) don't support private WebSocket
- **Priority 1 Custom Implementations** (10 exchanges):
  - ✅ Upbit: HMAC-SHA256 signing, watch_orders, watch_my_trades, watch_balance
  - ✅ Bithumb: HMAC-SHA256 signing, private channel parsers
  - ✅ HitBTC: HMAC-SHA256 signing, private channel parsers
  - ✅ Bitrue: HMAC-SHA256 signing, private channel parsers
  - ✅ CoinEx: HMAC-SHA256 signing, private channel parsers
  - ✅ HashKey: HMAC-SHA256 signing, private channel parsers
  - ✅ LBank: MD5 signing, private channel parsers (11 tests)
  - ✅ ProBit: OAuth2 Access Token authentication (8 tests)
  - ✅ WhiteBIT: HMAC-SHA512 signing, private channel parsers (9 tests)
  - ✅ XT: HMAC-SHA256 signing, private channel parsers (10 tests)
- Added private data types: `*OrderUpdateData`, `*BalanceUpdateData` for each exchange
- Added parsing methods: `parse_order()`, `parse_balance()` for unified type conversion
- Added `ws_authenticate()` method for WebSocket authentication
- All implementations include comprehensive tests
- **Total: 32+ exchanges with private channel support**

### 2025-12-31
- Added Backpack REST and WebSocket implementation (Solana-based exchange)
- Added Bullish WebSocket implementation
- Added OneTrading WebSocket implementation
- Added Hashkey WebSocket implementation
- Added Exmo WebSocket implementation
- Total: 61 REST, 51 WebSocket implementations

### 2025-12-28 (Phase 1-3 Complete)
- Phase 1 Complete: Deribit WS, Hyperliquid WS, BitMEX WS, Bitstamp WS
- Phase 2 Complete: Coinbase Advanced, Coinbase Exchange, Coinbase International
- Phase 3 Complete: BitMart, Blockchain.com, CEX.io, Blofin (BTSE not in reference)
- All WebSocket implementations verified working
- Total: 60 REST, 44 WebSocket implementations

### 2025-12-27
- Completed Phase 5 WebSocket implementations
- Added: Woo WS, Bitvavo WS, WhiteBit WS, Poloniex WS, Ascendex WS
- Added: HitBTC REST, Kraken Futures REST, KuCoin Futures REST
- Fixed all compilation errors and warnings
