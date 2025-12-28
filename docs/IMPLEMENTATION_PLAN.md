# CCXT-Rust Implementation Plan

> Created: 2025-12-28

## Overview

This document outlines the phased implementation plan for completing the CCXT-Rust porting project.

### Current Status
- **REST API**: 59/109 (54%) implemented
- **WebSocket**: 36/77 (47%) implemented
- **Tests**: 394 passing, 0 warnings

---

## Phase 1: High Priority WebSocket

**Goal**: Add WebSocket support to high-volume derivatives and major exchanges

### 1.1 Deribit WebSocket
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/deribit_ws.rs` |
| **Reference** | `ccxt-reference/js/src/pro/deribit.js` |
| **Estimated Lines** | ~1,500 |
| **Priority** | Critical |
| **Features** | Options/Futures specialized |

**Required Methods**:
- `watch_ticker` - Real-time ticker updates
- `watch_order_book` - Order book streaming
- `watch_trades` - Trade feed
- `watch_orders` - Order status updates
- `watch_my_trades` - Personal trade history
- `watch_positions` - Position updates (derivatives)

**WebSocket Details**:
```
URL: wss://www.deribit.com/ws/api/v2
Auth: JSON-RPC with API credentials
Format: JSON-RPC 2.0
```

---

### 1.2 Hyperliquid WebSocket
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/hyperliquid_ws.rs` |
| **Reference** | `ccxt-reference/js/src/pro/hyperliquid.js` |
| **Estimated Lines** | ~1,800 |
| **Priority** | Critical |
| **Features** | DEX, L1 blockchain |

**Required Methods**:
- `watch_order_book` - L2 order book
- `watch_trades` - Trade stream
- `watch_orders` - Order updates
- `watch_my_trades` - User trades
- `watch_positions` - Position tracking

**WebSocket Details**:
```
URL: wss://api.hyperliquid.xyz/ws
Auth: Signature-based
Format: Custom JSON
```

---

### 1.3 BitMEX WebSocket
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/bitmex_ws.rs` |
| **Reference** | `ccxt-reference/js/src/pro/bitmex.js` |
| **Estimated Lines** | ~1,200 |
| **Priority** | High |
| **Features** | Derivatives trading |

**Required Methods**:
- `watch_ticker` - Instrument updates
- `watch_order_book` - Order book L2
- `watch_trades` - Trade stream
- `watch_orders` - Order lifecycle
- `watch_positions` - Position updates

**WebSocket Details**:
```
URL: wss://www.bitmex.com/realtime
Auth: API key + signature
Format: JSON
```

---

### 1.4 Bitstamp WebSocket
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/bitstamp_ws.rs` |
| **Reference** | `ccxt-reference/js/src/pro/bitstamp.js` |
| **Estimated Lines** | ~1,000 |
| **Priority** | High |
| **Features** | Major fiat gateway |

**Required Methods**:
- `watch_ticker` - Live ticker
- `watch_order_book` - Order book feed
- `watch_trades` - Trade stream
- `watch_orders` - Order updates (private)

**WebSocket Details**:
```
URL: wss://ws.bitstamp.net
Auth: API key for private channels
Format: JSON (Pusher-like)
```

---

### Phase 1 Checklist
- [ ] Deribit WebSocket implementation
- [ ] Hyperliquid WebSocket implementation
- [ ] BitMEX WebSocket implementation
- [ ] Bitstamp WebSocket implementation
- [ ] Integration tests for all 4
- [ ] Update mod.rs exports

**Estimated Total Lines**: ~5,500

---

## Phase 2: Coinbase Family

**Goal**: Complete Coinbase ecosystem support

### 2.1 CoinbaseAdvanced (Alias)
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/coinbase_advanced.rs` |
| **Reference** | `ccxt-reference/ts/src/coinbaseadvanced.ts` |
| **Estimated Lines** | ~50 |
| **Priority** | Trivial |
| **Features** | Simple alias of Coinbase |

**Implementation**:
```rust
// Minimal implementation - wraps existing Coinbase
pub struct CoinbaseAdvanced(Coinbase);

impl Exchange for CoinbaseAdvanced {
    fn id(&self) -> ExchangeId { ExchangeId::CoinbaseAdvanced }
    // Delegate all methods to inner Coinbase
}
```

---

### 2.2 CoinbaseExchange
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/coinbase_exchange.rs` |
| **Reference** | `ccxt-reference/go/ccxt/coinbaseexchange.go` |
| **Estimated Lines** | ~2,500 |
| **Priority** | Major |
| **Features** | Separate API structure |

**API Endpoints**:
```
Base URL: https://api.exchange.coinbase.com
Auth: CB-ACCESS-KEY, CB-ACCESS-SIGN, CB-ACCESS-TIMESTAMP
```

**Required Methods**:
- `fetch_markets` - Market listing
- `fetch_ticker` / `fetch_tickers` - Price data
- `fetch_order_book` - Order book
- `fetch_trades` - Recent trades
- `create_order` - Order placement
- `cancel_order` - Order cancellation
- `fetch_orders` / `fetch_open_orders` - Order queries
- `fetch_balance` - Account balance
- `fetch_my_trades` - Trade history

**WebSocket** (Optional):
- `watch_ticker`
- `watch_order_book`
- `watch_trades`

---

### 2.3 CoinbaseInternational
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/coinbase_international.rs` |
| **Reference** | `ccxt-reference/go/ccxt/coinbaseinternational.go` |
| **Estimated Lines** | ~2,900 |
| **Priority** | Major |
| **Features** | International compliance |

**API Endpoints**:
```
Base URL: https://api.international.coinbase.com
Auth: Similar to CoinbaseExchange
Features: Derivatives, margin trading
```

**Required Methods**:
- All standard REST methods
- Derivatives-specific methods
- Margin trading methods

---

### Phase 2 Checklist
- [ ] CoinbaseAdvanced alias implementation
- [ ] CoinbaseExchange REST implementation
- [ ] CoinbaseExchange WebSocket (optional)
- [ ] CoinbaseInternational REST implementation
- [ ] Add ExchangeId variants
- [ ] Integration tests

**Estimated Total Lines**: ~5,500

---

## Phase 3: Medium Priority REST

**Goal**: Add popular mid-tier exchanges

### 3.1 BitMart
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/bitmart.rs` |
| **Reference** | `ccxt-reference/ts/src/bitmart.ts` |
| **Estimated Lines** | ~2,000 |
| **Priority** | Medium |

**Features**:
- Spot and futures trading
- Standard REST API
- WebSocket available

---

### 3.2 BTSE
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/btse.rs` |
| **Reference** | `ccxt-reference/ts/src/btse.ts` |
| **Estimated Lines** | ~1,800 |
| **Priority** | Medium |

**Features**:
- Derivatives trading
- Multi-currency settlement
- Advanced order types

---

### 3.3 Blockchain.com
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/blockchaincom.rs` |
| **Reference** | `ccxt-reference/ts/src/blockchaincom.ts` |
| **Estimated Lines** | ~1,500 |
| **Priority** | Medium |

**Features**:
- 30+ blockchain networks
- Beneficiary ID system
- Institutional focus

---

### 3.4 CEX.io
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/cex.rs` |
| **Reference** | `ccxt-reference/ts/src/cex.ts` |
| **Estimated Lines** | ~1,800 |
| **Priority** | Medium |

**Features**:
- Multi-account system
- Fiat gateway (USD, EUR, GBP)
- Credit card purchases

---

### 3.5 Blofin
| Item | Details |
|------|---------|
| **File** | `src/exchanges/foreign/blofin.rs` |
| **Reference** | `ccxt-reference/ts/src/blofin.ts` |
| **Estimated Lines** | ~1,500 |
| **Priority** | Medium |

**Features**:
- Options platform specialized
- Copy trading
- Grid trading

---

### Phase 3 Checklist
- [ ] BitMart REST implementation
- [ ] BTSE REST implementation
- [ ] Blockchain.com REST implementation
- [ ] CEX.io REST implementation
- [ ] Blofin REST implementation
- [ ] Add ExchangeId variants
- [ ] Integration tests

**Estimated Total Lines**: ~8,600

---

## Phase 4: Regional Exchanges

**Goal**: Regional market coverage based on demand

### 4.1 India Region
| Exchange | Lines | Notes |
|----------|-------|-------|
| CoinDCX | ~1,200 | Major Indian exchange |
| BitBNS | ~900 | Local focus |
| WazirX | ~1,000 | Binance partnership |

### 4.2 Japan Region
| Exchange | Lines | Notes |
|----------|-------|-------|
| BitBank | ~1,100 | Regulated |
| Liquid | ~1,100 | (formerly Quoine) |
| Zaif | ~900 | Legacy exchange |

### 4.3 Europe Region
| Exchange | Lines | Notes |
|----------|-------|-------|
| BL3P | ~600 | Netherlands |
| CoinMate | ~900 | Czech Republic |
| Zonda | ~900 | Poland |
| Paymium | ~700 | France |

### 4.4 Americas Region
| Exchange | Lines | Notes |
|----------|-------|-------|
| Ripio | ~800 | Argentina |
| NovaDAX | ~1,000 | Brazil |

### 4.5 Australia Region
| Exchange | Lines | Notes |
|----------|-------|-------|
| CoinSpot | ~700 | Consumer focus |
| IndependentReserve | ~900 | Institutional |

### Phase 4 Checklist
- [ ] Priority exchanges based on user demand
- [ ] Implement in batches of 3-5
- [ ] Add ExchangeId variants
- [ ] Integration tests

**Estimated Total Lines**: ~12,000+

---

## Summary Timeline

| Phase | Exchanges | Est. Lines | Priority |
|-------|-----------|------------|----------|
| 1 | 4 WebSocket | ~5,500 | Critical |
| 2 | 3 Coinbase | ~5,500 | High |
| 3 | 5 REST | ~8,600 | Medium |
| 4 | 15+ Regional | ~12,000+ | Low |
| **Total** | **27+** | **~31,600+** | - |

---

## Implementation Guidelines

### Code Standards
1. Follow existing patterns in `binance.rs` / `binance_ws.rs`
2. Use `Decimal` for all price/quantity values
3. Implement `Exchange` trait for REST
4. Implement `WsExchange` trait for WebSocket
5. Add comprehensive error handling

### Testing Requirements
1. Unit tests for parsing functions
2. Mock API response tests
3. WebSocket message handling tests
4. Add to integration test suite

### Documentation
1. Update `PORTING_STATUS.md` after each completion
2. Add exchange-specific notes if needed
3. Update `mod.rs` exports

### File Naming Convention
```
REST:      {exchange}.rs
WebSocket: {exchange}_ws.rs
```

---

## Next Steps

1. **Confirm priorities** with stakeholder
2. **Start Phase 1** with Deribit WebSocket
3. **Track progress** in `PORTING_STATUS.md`
4. **Review and test** after each implementation
