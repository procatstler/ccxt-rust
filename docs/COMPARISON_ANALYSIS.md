# CCXT Reference vs CCXT-Rust 비교 분석

> 분석일: 2025년 12월
>
> 이 문서는 CCXT TypeScript 레퍼런스 구현과 ccxt-rust 프로젝트 간의 상세 비교 분석을 제공합니다.

## 목차

1. [전체 규모 비교](#1-전체-규모-비교)
2. [구현된 기능](#2-구현된-기능)
3. [미구현 API 메서드](#3-미구현-api-메서드)
4. [WebSocket 지원 현황](#4-websocket-지원-현황)
5. [헬퍼 함수 비교](#5-헬퍼-함수-비교)
6. [타입 시스템 비교](#6-타입-시스템-비교)
7. [에러 처리 비교](#7-에러-처리-비교)

---

## 1. 전체 규모 비교

### 1.1 수치 비교

| 항목 | CCXT Reference (TS) | CCXT-Rust | 완성도 |
|------|---------------------|-----------|--------|
| **Exchange 메서드** | 257개 async 메서드 | ~50개 async 메서드 | 19% |
| **거래소 구현** | 100+ 거래소 | 10개 거래소 | 10% |
| **WebSocket 구현** | 77개 거래소 | 1개 (Binance) | 1% |
| **타입 정의** | 652줄 | 5,173줄 | 100%+ |
| **에러 타입** | 20+ 타입 | 28 타입 | 100%+ |

### 1.2 파일 크기 비교

```
CCXT Reference:
├── base/Exchange.ts     401KB (핵심 기능)
├── binance.ts           739KB (가장 큰 거래소)
├── pro/ (WebSocket)     81,817줄 총합

CCXT-Rust:
├── src/types/           5,173줄
├── src/exchanges/       ~10,000줄
├── src/client/          ~1,500줄
└── src/errors.rs        ~500줄
```

---

## 2. 구현된 기능

### 2.1 지원 거래소

#### 해외 거래소 (7개)
| 거래소 | REST API | WebSocket | 선물/마진 |
|--------|----------|-----------|-----------|
| Binance | ✅ | ✅ | ✅ |
| OKX | ✅ | ❌ | ✅ |
| Bybit | ✅ | ❌ | ✅ |
| Gate.io | ✅ | ❌ | ❌ |
| Kucoin | ✅ | ❌ | ❌ |
| Bitget | ✅ | ❌ | ❌ |

#### 국내 거래소 (3개)
| 거래소 | REST API | WebSocket | 비고 |
|--------|----------|-----------|------|
| Upbit | ✅ | ❌ | JWT 인증 |
| Bithumb | ✅ | ❌ | HMAC-SHA512 |
| Coinone | ✅ | ❌ | HMAC-SHA512 |

### 2.2 구현된 Exchange Trait 메서드

#### Public API (9개)
```rust
// 시장 데이터
async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>>;
async fn fetch_markets(&self) -> CcxtResult<Vec<Market>>;
async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, Currency>>;
async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker>;
async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>>;
async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook>;
async fn fetch_trades(&self, symbol: &str, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>>;
async fn fetch_ohlcv(&self, symbol: &str, timeframe: Timeframe, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<OHLCV>>;
```

#### Private Trading API (14개)
```rust
// 잔고 및 주문
async fn fetch_balance(&self) -> CcxtResult<Balances>;
async fn create_order(&self, symbol: &str, order_type: OrderType, side: OrderSide, amount: Decimal, price: Option<Decimal>) -> CcxtResult<Order>;
async fn create_limit_order(&self, symbol: &str, side: OrderSide, amount: Decimal, price: Decimal) -> CcxtResult<Order>;
async fn create_market_order(&self, symbol: &str, side: OrderSide, amount: Decimal) -> CcxtResult<Order>;
async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;
async fn cancel_orders(&self, ids: &[&str], symbol: &str) -> CcxtResult<Vec<Order>>;
async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;
async fn fetch_open_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>>;
async fn fetch_closed_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>>;
async fn fetch_canceled_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>>;
async fn fetch_my_trades(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>>;
```

#### Deposit/Withdrawal API (6개)
```rust
async fn fetch_deposits(&self, code: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Transaction>>;
async fn fetch_withdrawals(&self, code: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Transaction>>;
async fn withdraw(&self, code: &str, amount: Decimal, address: &str, tag: Option<&str>) -> CcxtResult<Transaction>;
async fn fetch_deposit_address(&self, code: &str, network: Option<&str>) -> CcxtResult<DepositAddress>;
async fn transfer(&self, code: &str, amount: Decimal, from_account: &str, to_account: &str) -> CcxtResult<TransferEntry>;
async fn fetch_ledger(&self, code: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<LedgerEntry>>;
```

#### Derivatives API (25개+)
```rust
// 포지션
async fn fetch_position(&self, symbol: &str) -> CcxtResult<Position>;
async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>>;
async fn close_position(&self, symbol: &str, side: Option<PositionSide>) -> CcxtResult<Order>;
async fn close_all_positions(&self) -> CcxtResult<Vec<Order>>;

// 레버리지
async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage>;
async fn fetch_leverage(&self, symbol: &str) -> CcxtResult<Leverage>;
async fn fetch_leverage_tiers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Vec<LeverageTier>>>;

// 마진
async fn set_margin_mode(&self, margin_mode: MarginMode, symbol: &str) -> CcxtResult<MarginModeInfo>;
async fn fetch_margin_mode(&self, symbol: &str) -> CcxtResult<MarginModeInfo>;
async fn borrow_margin(&self, code: &str, amount: Decimal, symbol: Option<&str>) -> CcxtResult<BorrowInterest>;
async fn repay_margin(&self, code: &str, amount: Decimal, symbol: Option<&str>) -> CcxtResult<BorrowInterest>;

// 펀딩
async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate>;
async fn fetch_funding_rates(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, FundingRate>>;
async fn fetch_funding_rate_history(&self, symbol: &str, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<FundingRateHistory>>;

// 기타
async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest>;
async fn fetch_liquidations(&self, symbol: &str, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Liquidation>>;
async fn fetch_my_liquidations(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Liquidation>>;
```

---

## 3. 미구현 API 메서드

### 3.1 주문 관련 (Order Management) - 🔴 높은 우선순위

| 메서드 | 설명 | CCXT Reference | ccxt-rust |
|--------|------|----------------|-----------|
| `editOrder()` | 주문 수정 | ✅ | ❌ |
| `cancelAllOrders()` | 모든 주문 취소 | ✅ | ❌ |
| `createOrders()` | 복수 주문 생성 | ✅ | ❌ |
| `createStopOrder()` | 스탑 주문 | ✅ | ❌ |
| `createStopLimitOrder()` | 스탑 리밋 주문 | ✅ | ❌ |
| `createStopMarketOrder()` | 스탑 마켓 주문 | ✅ | ❌ |
| `createTakeProfitOrder()` | 익절 주문 | ✅ | ❌ |
| `createStopLossOrder()` | 손절 주문 | ✅ | ❌ |
| `createPostOnlyOrder()` | Post-only 주문 | ✅ | ❌ |
| `createReduceOnlyOrder()` | Reduce-only 주문 | ✅ | ❌ |
| `createOrderWithTakeProfitAndStopLoss()` | TP/SL 동시 설정 | ✅ | ❌ |
| `cancelOrdersForSymbols()` | 심볼별 취소 | ✅ | ❌ |
| `fetchOrderTrades()` | 주문별 체결 내역 | ✅ | ❌ |

### 3.2 계정/수수료 관련 - 🔴 높은 우선순위

| 메서드 | 설명 | CCXT Reference | ccxt-rust |
|--------|------|----------------|-----------|
| `fetchTradingFees()` | 거래 수수료 조회 | ✅ | ❌ |
| `fetchTradingFee()` | 개별 심볼 수수료 | ✅ | ❌ |
| `fetchDepositWithdrawFees()` | 입출금 수수료 | ✅ | ❌ |
| `fetchTransfers()` | 내부 이체 내역 | ✅ | ❌ |
| `fetchTransactions()` | 모든 트랜잭션 | ✅ | ❌ |
| `fetchAccounts()` | 계정 목록 | ✅ | ⚠️ 기본 구현만 |

### 3.3 파생상품 관련 - 🟡 중간 우선순위

| 메서드 | 설명 | CCXT Reference | ccxt-rust |
|--------|------|----------------|-----------|
| `setPositionMode()` | Hedge/One-way 모드 | ✅ | ❌ |
| `addMargin()` | 마진 추가 | ✅ | ❌ |
| `reduceMargin()` | 마진 감소 | ✅ | ❌ |
| `setMargin()` | 마진 설정 | ✅ | ❌ |
| `fetchPositionHistory()` | 포지션 히스토리 | ✅ | ❌ |
| `fetchPositionsHistory()` | 전체 포지션 히스토리 | ✅ | ❌ |
| `fetchMarkOHLCV()` | Mark Price OHLCV | ✅ | ❌ |
| `fetchIndexOHLCV()` | Index Price OHLCV | ✅ | ❌ |
| `fetchPremiumIndexOHLCV()` | Premium Index OHLCV | ✅ | ❌ |
| `fetchLongShortRatio()` | 롱숏 비율 | ✅ | ❌ |
| `fetchLongShortRatioHistory()` | 롱숏 비율 히스토리 | ✅ | ❌ |
| `fetchMarginAdjustmentHistory()` | 마진 조정 히스토리 | ✅ | ❌ |

### 3.4 시장 데이터 관련 - 🟡 중간 우선순위

| 메서드 | 설명 | CCXT Reference | ccxt-rust |
|--------|------|----------------|-----------|
| `fetchTime()` | 서버 시간 조회 | ✅ | ❌ |
| `fetchStatus()` | 거래소 상태 | ✅ | ❌ |
| `fetchL3OrderBook()` | L3 호가창 | ✅ | ❌ |
| `fetchLastPrices()` | 최근가 | ✅ | ❌ |
| `fetchBidsAsks()` | 최우선 호가 | ✅ | ❌ |
| `fetchTradingLimits()` | 거래 한도 | ✅ | ❌ |

### 3.5 옵션 거래 관련 - 🟢 낮은 우선순위

| 메서드 | 설명 | CCXT Reference | ccxt-rust |
|--------|------|----------------|-----------|
| `fetchGreeks()` | 옵션 Greeks | ✅ | ❌ |
| `fetchAllGreeks()` | 전체 Greeks | ✅ | ❌ |
| `fetchOptionChain()` | 옵션 체인 | ✅ | ❌ |
| `fetchOption()` | 개별 옵션 | ✅ | ❌ |

### 3.6 변환/기타 - 🟢 낮은 우선순위

| 메서드 | 설명 | CCXT Reference | ccxt-rust |
|--------|------|----------------|-----------|
| `fetchConvertQuote()` | 변환 견적 | ✅ | ❌ |
| `fetchConvertCurrencies()` | 변환 가능 통화 | ✅ | ❌ |
| `signIn()` | 로그인 | ✅ | ❌ |
| `fetchPaymentMethods()` | 결제 방법 | ✅ | ❌ |

---

## 4. WebSocket 지원 현황

### 4.1 WsExchange Trait 정의 (ccxt-rust)

```rust
pub trait WsExchange: Exchange {
    // Public Streams
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_order_book_for_symbols(&self, symbols: &[&str], limit: Option<u32>) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_trades_for_symbols(&self, symbols: &[&str]) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<UnboundedReceiver<WsMessage>>;

    // Private Streams
    async fn watch_balance(&self) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<UnboundedReceiver<WsMessage>>;

    // Connection Management
    async fn ws_connect(&mut self) -> CcxtResult<()>;
    async fn ws_close(&mut self) -> CcxtResult<()>;
    async fn ws_is_connected(&self) -> bool;
    async fn ws_authenticate(&mut self) -> CcxtResult<()>;
}
```

### 4.2 구현 현황

| 거래소 | Public WS | Private WS | 상태 |
|--------|-----------|------------|------|
| Binance | ✅ | ✅ | 완료 |
| OKX | ❌ | ❌ | 미구현 |
| Bybit | ❌ | ❌ | 미구현 |
| Gate.io | ❌ | ❌ | 미구현 |
| Kucoin | ❌ | ❌ | 미구현 |
| Bitget | ❌ | ❌ | 미구현 |
| Upbit | ❌ | ❌ | 미구현 |
| Bithumb | ❌ | ❌ | 미구현 |
| Coinone | ❌ | ❌ | 미구현 |

### 4.3 CCXT Reference 미지원 WebSocket 메서드

| 메서드 | 설명 |
|--------|------|
| `watchPositions()` | 포지션 실시간 |
| `watchFundingRate()` | 펀딩비 실시간 |
| `watchFundingRates()` | 전체 펀딩비 실시간 |
| `watchLiquidations()` | 청산 실시간 |
| `watchMyLiquidations()` | 내 청산 실시간 |
| `watchMarkPrice()` | Mark Price 실시간 |
| `watchMarkPrices()` | 전체 Mark Price 실시간 |
| `unWatch*()` | 구독 해제 메서드들 |

---

## 5. 헬퍼 함수 비교

### 5.1 Safe* 헬퍼 함수 (CCXT Reference에만 존재)

```typescript
// 안전한 데이터 추출 - null/undefined 처리
safeString(obj, key, defaultValue)      // 문자열
safeString2(obj, key1, key2)            // 2개 키 시도
safeStringN(obj, keys[])                // N개 키 시도
safeStringLower(obj, key)               // 소문자로
safeStringUpper(obj, key)               // 대문자로

safeInteger(obj, key, defaultValue)     // 정수
safeInteger2(obj, key1, key2)
safeIntegerN(obj, keys[])
safeIntegerProduct(obj, key, factor)    // 정수 * 배수

safeFloat(obj, key, defaultValue)       // 실수
safeFloat2(obj, key1, key2)
safeFloatN(obj, keys[])

safeValue(obj, key, defaultValue)       // 임의 값
safeValue2(obj, key1, key2)
safeValueN(obj, keys[])

safeTimestamp(obj, key)                 // 타임스탬프
safeTimestamp2(obj, key1, key2)
safeTimestampN(obj, keys[])

// 시장/통화 안전 조회
safeCurrency(currencyId)
safeMarket(marketId)
safeSymbol(marketId)
```

### 5.2 Parse* 헬퍼 함수 (CCXT Reference에만 존재)

```typescript
// 데이터 파싱 - API 응답을 통일된 형식으로 변환
parseOrder(order, market)
parseTrade(trade, market)
parseTicker(ticker, market)
parseBalance(response)
parsePosition(position, market)
parseTransaction(transaction, currency)
parseLedgerEntry(item, currency)
parseOHLCV(ohlcv, market)
parseOrderBook(orderbook, symbol)
parseFundingRate(fundingRate, market)
parseOpenInterest(openInterest, market)
parseLiquidation(liquidation, market)
```

### 5.3 Precision 헬퍼 함수

```typescript
// CCXT Reference
amountToPrecision(symbol, amount)
priceToPrecision(symbol, price)
costToPrecision(symbol, cost)
currencyToPrecision(code, amount)
decimalToPrecision(value, roundingMode, precision)

// ccxt-rust (utils/precise.rs에 일부 구현)
Precise::mul(a, b)
Precise::div(a, b, precision)
Precise::add(a, b)
Precise::sub(a, b)
```

### 5.4 ccxt-rust 권장 구현

```rust
// 제안: safe_* 매크로/함수 구현
pub fn safe_string(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

pub fn safe_string2(obj: &Value, key1: &str, key2: &str) -> Option<String> {
    safe_string(obj, key1).or_else(|| safe_string(obj, key2))
}

pub fn safe_decimal(obj: &Value, key: &str) -> Option<Decimal> {
    obj.get(key)
        .and_then(|v| v.as_str().or_else(|| v.as_f64().map(|f| f.to_string()).as_deref()))
        .and_then(|s| Decimal::from_str(s).ok())
}
```

---

## 6. 타입 시스템 비교

### 6.1 핵심 타입 비교

| 타입 | CCXT Reference | ccxt-rust | 비고 |
|------|----------------|-----------|------|
| Market | ✅ 38 필드 | ✅ 40+ 필드 | 동등 |
| Ticker | ✅ 22 필드 | ✅ 22 필드 | 동등 |
| Order | ✅ 23 필드 | ✅ 25 필드 | 동등 |
| Trade | ✅ 13 필드 | ✅ 14 필드 | 동등 |
| Balance | ✅ 4 필드 | ✅ 4 필드 | 동등 |
| Position | ✅ 25 필드 | ✅ 25+ 필드 | 동등 |
| FundingRate | ✅ 15 필드 | ✅ 15+ 필드 | 동등 |
| Greeks | ✅ 18 필드 | ❌ | 미구현 |
| Option | ✅ 15 필드 | ❌ | 미구현 |
| Conversion | ✅ 10 필드 | ❌ | 미구현 |

### 6.2 Enum 비교

| Enum | CCXT Reference | ccxt-rust |
|------|----------------|-----------|
| OrderType | 8개 | 8개 ✅ |
| OrderSide | 2개 | 2개 ✅ |
| OrderStatus | 5개 | 5개 ✅ |
| MarketType | 7개 | 6개 (delivery 누락) |
| Timeframe | 동적 | 16개 고정 ✅ |
| ExchangeId | 동적 | 18개 ✅ |

---

## 7. 에러 처리 비교

### 7.1 에러 계층 구조

```
ccxt-rust CcxtError (28 타입)
├── Exchange 에러 (11개)
│   ├── ExchangeError
│   ├── AuthenticationError
│   ├── PermissionDenied
│   ├── AccountNotEnabled
│   ├── AccountSuspended
│   ├── ArgumentsRequired
│   ├── BadRequest
│   ├── BadSymbol
│   ├── OperationRejected
│   ├── NotSupported
│   └── InvalidProxySettings
├── Order 에러 (8개)
│   ├── InvalidOrder
│   ├── OrderNotFound
│   ├── OrderNotCached
│   ├── OrderImmediatelyFillable
│   ├── OrderNotFillable
│   ├── DuplicateOrderId
│   ├── ContractUnavailable
│   └── InsufficientFunds
├── Network 에러 (7개)
│   ├── NetworkError
│   ├── DDoSProtection
│   ├── RateLimitExceeded
│   ├── ExchangeNotAvailable
│   ├── RequestTimeout
│   ├── OnMaintenance
│   └── InvalidNonce
└── 기타 에러 (2개)
    ├── BadResponse
    └── NullResponse
```

### 7.2 에러 헬퍼 메서드

```rust
impl CcxtError {
    pub fn code(&self) -> &'static str;      // 에러 코드
    pub fn is_retryable(&self) -> bool;      // 재시도 가능 여부
    pub fn is_auth_error(&self) -> bool;     // 인증 에러 여부
    pub fn is_order_error(&self) -> bool;    // 주문 에러 여부
    pub fn is_network_error(&self) -> bool;  // 네트워크 에러 여부
}
```

---

## 결론

### 강점 (ccxt-rust)
1. **타입 안전성**: Rust의 강력한 타입 시스템 활용
2. **에러 처리**: 포괄적인 에러 타입 정의
3. **비동기 지원**: async/await 기반의 현대적 설계
4. **성능**: 컴파일 타임 최적화

### 개선 필요 영역
1. **WebSocket**: 대부분의 거래소에서 미구현
2. **주문 관리**: 고급 주문 타입 미지원
3. **헬퍼 함수**: safe*/parse* 함수 필요
4. **거래소 커버리지**: 10개 → 20개+ 확대 필요

### 다음 단계
자세한 개선 로드맵은 [IMPROVEMENT_ROADMAP.md](./IMPROVEMENT_ROADMAP.md)를 참조하세요.
