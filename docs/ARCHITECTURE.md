# CCXT-Rust 아키텍처

> 이 문서는 ccxt-rust 프로젝트의 전체 아키텍처와 디자인 결정을 설명합니다.

## 목차

1. [프로젝트 구조](#1-프로젝트-구조)
2. [핵심 컴포넌트](#2-핵심-컴포넌트)
3. [Trait 설계](#3-trait-설계)
4. [타입 시스템](#4-타입-시스템)
5. [에러 처리](#5-에러-처리)
6. [HTTP 클라이언트](#6-http-클라이언트)
7. [인증 체계](#7-인증-체계)
8. [WebSocket 아키텍처](#8-websocket-아키텍처)
9. [암호화 모듈](#9-암호화-모듈)

---

## 1. 프로젝트 구조

```
ccxt-rust/
├── src/
│   ├── lib.rs              # 라이브러리 진입점 및 re-exports
│   ├── errors.rs           # CcxtError 타입 정의
│   ├── macros.rs           # 공통 매크로 정의
│   │
│   ├── client/             # HTTP/WS 클라이언트
│   │   ├── mod.rs
│   │   ├── http.rs         # HTTP 클라이언트
│   │   ├── websocket.rs    # WebSocket 클라이언트
│   │   ├── config.rs       # 거래소 설정
│   │   ├── cache.rs        # 마켓 캐시
│   │   └── rate_limiter.rs # Rate limiting
│   │
│   ├── crypto/             # 암호화 모듈
│   │   ├── mod.rs
│   │   ├── common/         # 공통 암호화 유틸리티
│   │   │   ├── mod.rs
│   │   │   ├── traits.rs   # 서명 트레이트
│   │   │   ├── jwt.rs      # JWT 생성
│   │   │   ├── rsa.rs      # RSA 서명
│   │   │   └── totp.rs     # TOTP 생성
│   │   ├── evm/            # EVM 체인 지원
│   │   │   ├── mod.rs
│   │   │   ├── wallet.rs   # EVM 지갑
│   │   │   ├── keccak.rs   # Keccak 해시
│   │   │   ├── secp256k1.rs
│   │   │   └── eip712.rs   # EIP-712 서명
│   │   ├── cosmos/         # Cosmos SDK 지원
│   │   │   ├── mod.rs
│   │   │   ├── wallet.rs
│   │   │   ├── keys.rs
│   │   │   ├── address.rs
│   │   │   ├── signer.rs
│   │   │   ├── transaction.rs
│   │   │   └── protobuf.rs
│   │   └── starknet/       # Starknet 지원
│   │       ├── mod.rs
│   │       ├── wallet.rs
│   │       ├── account.rs
│   │       ├── curve.rs
│   │       ├── poseidon.rs
│   │       ├── typed_data.rs
│   │       └── paradex.rs
│   │
│   ├── types/              # 공통 타입 정의
│   │   ├── mod.rs
│   │   ├── exchange.rs     # Exchange trait 및 ExchangeId
│   │   ├── ws_exchange.rs  # WsExchange trait
│   │   ├── market.rs       # Market, Currency
│   │   ├── ticker.rs       # Ticker
│   │   ├── order.rs        # Order, OrderType, OrderSide
│   │   ├── trade.rs        # Trade
│   │   ├── balance.rs      # Balance, Balances
│   │   ├── position.rs     # Position, PositionSide
│   │   ├── orderbook.rs    # OrderBook, OrderBookEntry
│   │   ├── ohlcv.rs        # OHLCV, Timeframe
│   │   ├── transaction.rs  # Transaction, TransferEntry
│   │   ├── funding.rs      # FundingRate, FundingRateHistory
│   │   ├── leverage.rs     # Leverage, LeverageTier
│   │   ├── margin.rs       # MarginMode, BorrowInterest
│   │   ├── liquidation.rs  # Liquidation
│   │   ├── open_interest.rs # OpenInterest
│   │   ├── account.rs      # DepositAddress, Ledger
│   │   ├── convert.rs      # ConvertQuote, ConvertTrade
│   │   ├── fee.rs          # TradingFee, DepositWithdrawFee
│   │   ├── derivatives.rs  # Greeks, OptionChain
│   │   ├── currency.rs     # Currency
│   │   │
│   │   └── traits/         # Sub-traits (8개)
│   │       ├── mod.rs
│   │       ├── base.rs           # ExchangeBase
│   │       ├── public_market.rs  # PublicMarketApi
│   │       ├── spot_trading.rs   # SpotTradingApi
│   │       ├── account.rs        # AccountApi
│   │       ├── derivatives.rs    # DerivativesApi
│   │       ├── margin.rs         # MarginApi
│   │       ├── options.rs        # OptionsApi
│   │       └── convert.rs        # ConvertApi
│   │
│   ├── exchanges/          # 거래소 구현체
│   │   ├── mod.rs
│   │   ├── cex/            # 중앙화 거래소 (109개)
│   │   │   ├── mod.rs
│   │   │   ├── binance.rs
│   │   │   ├── binance_ws.rs
│   │   │   ├── okx.rs
│   │   │   ├── bybit.rs
│   │   │   ├── upbit.rs
│   │   │   ├── bithumb.rs
│   │   │   └── ...
│   │   │
│   │   └── dex/            # 탈중앙화 거래소 (8개)
│   │       ├── mod.rs
│   │       ├── hyperliquid.rs
│   │       ├── dydx.rs
│   │       ├── dydxv4.rs
│   │       ├── apex.rs
│   │       ├── paradex.rs
│   │       └── ...
│   │
│   └── utils/              # 유틸리티
│       ├── mod.rs
│       ├── safe.rs         # safe_* 헬퍼 함수 (30+)
│       ├── parse.rs        # parse_* 헬퍼 함수 (20+)
│       ├── precise.rs      # 정밀 연산
│       ├── precision.rs    # 정밀도 계산
│       ├── crypto.rs       # 암호화 유틸리티
│       └── time.rs         # 시간 유틸리티
│
├── tests/                  # 테스트
├── examples/               # 예제 코드
└── docs/                   # 문서
```

---

## 2. 핵심 컴포넌트

### 2.1 의존성 관계

```
┌─────────────────────────────────────────────────────────────┐
│                        lib.rs                                │
│  (Public API: Exchange, WsExchange, types, errors)          │
└─────────────────────────────────────────────────────────────┘
                              │
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
│ types/exchange  │  │types/ws_exchange│  │    errors.rs    │
│ (Exchange trait)│  │(WsExchange trait)│  │  (CcxtError)   │
└─────────────────┘  └─────────────────┘  └─────────────────┘
          │                   │                   ▲
          │                   │                   │
          ▼                   ▼                   │
┌─────────────────────────────────────────────────┴───────────┐
│                     exchanges/                               │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │   CEX    │  │   DEX    │  │          │  │          │    │
│  │ Binance  │  │Hyperliquid│ │  Upbit   │  │   ...    │    │
│  │ OKX, ... │  │ dYdX,... │  │          │  │          │    │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘    │
└─────────────────────────────────────────────────────────────┘
          │                              │
          ▼                              ▼
┌─────────────────────────┐    ┌─────────────────────────────┐
│       client/           │    │          crypto/            │
│  HttpClient, WebSocket  │    │  EVM, Cosmos, Starknet     │
│  RateLimiter, Cache     │    │  JWT, RSA, TOTP            │
└─────────────────────────┘    └─────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────┐
│                      types/                                  │
│  Market, Ticker, Order, Trade, Balance, Position, ...       │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 데이터 흐름

```
사용자 코드
    │
    ▼
Exchange Trait 메서드 호출
    │
    ▼
거래소별 구현체 (Binance, OKX, ...)
    │
    ├─► 요청 생성 (params, headers, body)
    │
    ├─► 인증 서명 (HMAC-SHA256/512, JWT, EIP-712, ...)
    │
    ▼
HttpClient
    │
    ├─► Rate Limiter 체크
    │
    ├─► HTTP 요청 전송
    │
    ├─► 응답 수신
    │
    ▼
응답 파싱 (serde_json)
    │
    ├─► 에러 체크 (거래소별 에러 코드 매핑)
    │
    ▼
통일된 타입 반환 (Order, Trade, ...)
```

---

## 3. Trait 설계

### 3.1 Sub-traits 아키텍처

Exchange 기능을 8개의 논리적 sub-traits로 분리하여 관리성과 유지보수성을 향상:

```rust
// types/traits/mod.rs
pub trait ExchangeBase: Send + Sync { ... }     // 메타데이터, 마켓 로딩
pub trait PublicMarketApi: Send + Sync { ... }  // 공개 시장 데이터
pub trait SpotTradingApi: Send + Sync { ... }   // 현물 거래
pub trait AccountApi: Send + Sync { ... }       // 계정/지갑 관리
pub trait DerivativesApi: Send + Sync { ... }   // 선물/무기한
pub trait MarginApi: Send + Sync { ... }        // 마진 거래
pub trait OptionsApi: Send + Sync { ... }       // 옵션 거래
pub trait ConvertApi: Send + Sync { ... }       // 통화 변환
```

### 3.2 ExchangeBase Trait

```rust
#[async_trait]
pub trait ExchangeBase: Send + Sync {
    fn id(&self) -> ExchangeId;
    fn name(&self) -> &str;
    fn has(&self) -> ExchangeFeatures;

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>>;
    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>>;
    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, Currency>>;
}
```

### 3.3 PublicMarketApi Trait

```rust
#[async_trait]
pub trait PublicMarketApi: Send + Sync {
    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker>;
    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>>;
    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook>;
    async fn fetch_trades(&self, symbol: &str, ...) -> CcxtResult<Vec<Trade>>;
    async fn fetch_ohlcv(&self, symbol: &str, ...) -> CcxtResult<Vec<OHLCV>>;
    async fn fetch_time(&self) -> CcxtResult<i64>;
    async fn fetch_status(&self) -> CcxtResult<ExchangeStatus>;
}
```

### 3.4 SpotTradingApi Trait

```rust
#[async_trait]
pub trait SpotTradingApi: Send + Sync {
    async fn fetch_balance(&self) -> CcxtResult<Balances>;
    async fn create_order(&self, ...) -> CcxtResult<Order>;
    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;
    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>>;
    async fn edit_order(&self, ...) -> CcxtResult<Order>;
    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;
    async fn fetch_open_orders(&self, ...) -> CcxtResult<Vec<Order>>;
    async fn fetch_closed_orders(&self, ...) -> CcxtResult<Vec<Order>>;
    async fn fetch_my_trades(&self, ...) -> CcxtResult<Vec<Trade>>;
    async fn fetch_trading_fees(&self) -> CcxtResult<HashMap<String, TradingFee>>;

    // 고급 주문 타입
    async fn create_stop_order(&self, ...) -> CcxtResult<Order>;
    async fn create_stop_limit_order(&self, ...) -> CcxtResult<Order>;
    async fn create_take_profit_order(&self, ...) -> CcxtResult<Order>;
    async fn create_stop_loss_order(&self, ...) -> CcxtResult<Order>;
}
```

### 3.5 DerivativesApi Trait

```rust
#[async_trait]
pub trait DerivativesApi: Send + Sync {
    async fn fetch_position(&self, symbol: &str) -> CcxtResult<Position>;
    async fn fetch_positions(&self, ...) -> CcxtResult<Vec<Position>>;
    async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage>;
    async fn set_margin_mode(&self, ...) -> CcxtResult<MarginModeInfo>;
    async fn set_position_mode(&self, ...) -> CcxtResult<PositionModeInfo>;
    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate>;
    async fn fetch_funding_rate_history(&self, ...) -> CcxtResult<Vec<FundingRateHistory>>;
    async fn fetch_leverage_tiers(&self, ...) -> CcxtResult<HashMap<String, Vec<LeverageTier>>>;
    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest>;
    async fn fetch_liquidations(&self, ...) -> CcxtResult<Vec<Liquidation>>;
}
```

### 3.6 ConvertApi Trait

```rust
#[async_trait]
pub trait ConvertApi: Send + Sync {
    async fn fetch_convert_currencies(&self) -> CcxtResult<Vec<ConvertCurrencyPair>>;
    async fn fetch_convert_quote(&self, from: &str, to: &str, amount: Decimal) -> CcxtResult<ConvertQuote>;
    async fn create_convert_trade(&self, quote_id: &str) -> CcxtResult<ConvertTrade>;
    async fn fetch_convert_trade(&self, id: &str) -> CcxtResult<ConvertTrade>;
    async fn fetch_convert_trade_history(&self, ...) -> CcxtResult<Vec<ConvertTrade>>;
}
```

### 3.7 WsExchange Trait

```rust
#[async_trait]
pub trait WsExchange: Exchange {
    // === Public Streams ===
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<Receiver<WsMessage>>;

    // === Private Streams ===
    async fn watch_balance(&self) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<Receiver<WsMessage>>;
    async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Receiver<WsMessage>>;

    // === Connection Management ===
    async fn ws_connect(&mut self) -> CcxtResult<()>;
    async fn ws_close(&mut self) -> CcxtResult<()>;
    fn ws_is_connected(&self) -> bool;
}
```

---

## 4. 타입 시스템

### 4.1 핵심 타입 구조

```rust
// Market - 거래쌍 정보
pub struct Market {
    pub id: String,           // 거래소 내부 ID (예: "BTCUSDT")
    pub symbol: String,       // 통일된 심볼 (예: "BTC/USDT")
    pub base: String,         // 기준 통화 (예: "BTC")
    pub quote: String,        // 견적 통화 (예: "USDT")
    pub base_id: String,      // 거래소 기준 통화 ID
    pub quote_id: String,     // 거래소 견적 통화 ID
    pub active: bool,         // 거래 가능 여부
    pub market_type: MarketType, // spot, future, swap, option
    pub precision: MarketPrecision,
    pub limits: MarketLimits,
    // ...
}

// Order - 주문 정보
pub struct Order {
    pub id: String,
    pub client_order_id: Option<String>,
    pub symbol: String,
    pub order_type: OrderType,
    pub side: OrderSide,
    pub status: OrderStatus,
    pub price: Option<Decimal>,
    pub amount: Decimal,
    pub filled: Decimal,
    pub remaining: Decimal,
    pub cost: Decimal,
    pub average: Option<Decimal>,
    pub fee: Option<Fee>,
    pub timestamp: Option<i64>,
    // ...
}

// Position - 포지션 정보 (선물/마진)
pub struct Position {
    pub symbol: String,
    pub side: PositionSide,
    pub contracts: Decimal,
    pub entry_price: Decimal,
    pub mark_price: Option<Decimal>,
    pub liquidation_price: Option<Decimal>,
    pub unrealized_pnl: Option<Decimal>,
    pub leverage: Option<Decimal>,
    pub margin_mode: MarginMode,
    // ...
}
```

### 4.2 Enum 설계

```rust
// 주문 타입
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
    StopMarket,
    StopLimit,
    StopLoss,
    StopLossLimit,
    TakeProfit,
    TakeProfitLimit,
    TakeProfitMarket,
    TrailingStopMarket,
    LimitMaker,
}

// 타임프레임
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeframe {
    Second1,
    Minute1,
    Minute3,
    Minute5,
    Minute15,
    Minute30,
    Hour1,
    Hour2,
    Hour3,
    Hour4,
    Hour6,
    Hour8,
    Hour12,
    Day1,
    Day3,
    Week1,
    Month1,
}
```

---

## 5. 에러 처리

### 5.1 CcxtError 구조

```rust
#[derive(Debug, Error)]
pub enum CcxtError {
    // === Exchange 에러 ===
    #[error("Exchange error: {message}")]
    ExchangeError { message: String },

    #[error("Authentication error: {message}")]
    AuthenticationError { message: String },

    #[error("Not supported: {feature}")]
    NotSupported { feature: String },

    // === Order 에러 ===
    #[error("Invalid order: {message}")]
    InvalidOrder { message: String },

    #[error("Order not found: {order_id}")]
    OrderNotFound { order_id: String },

    #[error("Insufficient funds: {message}")]
    InsufficientFunds { message: String },

    // === Network 에러 ===
    #[error("Network error: {message}")]
    NetworkError { message: String },

    #[error("Rate limit exceeded: {message}")]
    RateLimitExceeded { message: String },

    // ...
}
```

### 5.2 에러 헬퍼 메서드

```rust
impl CcxtError {
    pub fn code(&self) -> &'static str { ... }
    pub fn is_retryable(&self) -> bool { ... }
    pub fn is_auth_error(&self) -> bool { ... }
}
```

---

## 6. HTTP 클라이언트

### 6.1 HttpClient 구조

```rust
pub struct HttpClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpClient {
    pub fn new(base_url: &str, timeout_ms: Option<u64>) -> Self;

    pub async fn get<T: DeserializeOwned>(&self, path: &str, ...) -> CcxtResult<T>;
    pub async fn post<T: DeserializeOwned>(&self, path: &str, ...) -> CcxtResult<T>;
    pub async fn delete<T: DeserializeOwned>(&self, path: &str, ...) -> CcxtResult<T>;
    pub async fn put<T: DeserializeOwned>(&self, path: &str, ...) -> CcxtResult<T>;
}
```

### 6.2 Rate Limiter

```rust
pub struct RateLimiter {
    requests_per_second: f64,
    last_request: Instant,
}

impl RateLimiter {
    pub fn new(requests_per_second: f64) -> Self;
    pub async fn wait(&mut self);
}
```

---

## 7. 인증 체계

### 7.1 인증 방식별 분류

| 방식 | 거래소 | 설명 |
|------|--------|------|
| HMAC-SHA256 | Binance, OKX, Gate.io | 표준 서명 |
| HMAC-SHA256 + Passphrase | Kucoin, Bitget | 추가 암호 |
| HMAC-SHA512 | Bithumb, Coinone | SHA512 해시 |
| JWT | Upbit, Coinbase | JSON Web Token |
| Ed25519 | Coinbase International | EdDSA 서명 |
| RSA | 일부 거래소 | RSA 서명 |
| EIP-712 | Hyperliquid, Paradex | Ethereum 서명 |
| Cosmos SDK | dYdX v4 | Cosmos 트랜잭션 |

### 7.2 HMAC-SHA256 구현 예시 (Binance)

```rust
impl Binance {
    fn sign(&self, params: &str) -> CcxtResult<String> {
        let api_secret = self.config.api_secret.as_ref()?;
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())?;
        mac.update(params.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }
}
```

---

## 8. WebSocket 아키텍처

### 8.1 WsMessage 타입

```rust
pub enum WsMessage {
    Ticker(WsTickerEvent),
    OrderBook(WsOrderBookEvent),
    Trade(WsTradeEvent),
    Ohlcv(WsOhlcvEvent),
    Order(WsOrderEvent),
    Balance(WsBalanceEvent),
    Position(WsPositionEvent),
    MyTrade(WsMyTradeEvent),
    Liquidation(WsLiquidationEvent),
    Connected,
    Disconnected,
    Error(String),
    Ping,
    Pong,
}
```

### 8.2 재연결 전략

```rust
pub struct ReconnectConfig {
    pub max_retries: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub backoff_factor: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            max_retries: 10,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_factor: 2.0,
        }
    }
}
```

---

## 9. 암호화 모듈

### 9.1 crypto/common - 공통 암호화

```rust
// JWT 생성 (Upbit, Coinbase)
pub fn create_jwt(claims: &Value, secret: &str) -> CcxtResult<String>;

// RSA 서명
pub fn rsa_sign(message: &[u8], private_key: &str) -> CcxtResult<Vec<u8>>;

// TOTP 생성 (2FA)
pub fn generate_totp(secret: &str) -> CcxtResult<String>;
```

### 9.2 crypto/evm - EVM 체인 지원

Hyperliquid, Paradex 등 EVM 기반 DEX 지원:

```rust
// EIP-712 서명
pub fn sign_typed_data(typed_data: &TypedData, private_key: &str) -> CcxtResult<Signature>;

// Keccak256 해시
pub fn keccak256(data: &[u8]) -> [u8; 32];

// secp256k1 서명
pub fn sign_message(message: &[u8], private_key: &str) -> CcxtResult<Signature>;
```

### 9.3 crypto/cosmos - Cosmos SDK 지원

dYdX v4 등 Cosmos 기반 DEX 지원:

```rust
// Cosmos 트랜잭션 서명
pub fn sign_transaction(tx: &Transaction, wallet: &CosmosWallet) -> CcxtResult<SignedTx>;

// 주소 생성
pub fn derive_address(public_key: &PublicKey, prefix: &str) -> String;
```

### 9.4 crypto/starknet - Starknet 지원

Paradex 등 Starknet 기반 DEX 지원:

```rust
// Starknet 서명
pub fn sign_stark(message: &[u8], private_key: &StarkKey) -> CcxtResult<Signature>;

// Poseidon 해시
pub fn poseidon_hash(inputs: &[FieldElement]) -> FieldElement;
```

---

## 관련 문서

- [WEBSOCKET_GUIDE.md](./WEBSOCKET_GUIDE.md) - WebSocket 사용 가이드
- [MIGRATION_FROM_CCXT_JS.md](./MIGRATION_FROM_CCXT_JS.md) - CCXT JS에서 마이그레이션 가이드
