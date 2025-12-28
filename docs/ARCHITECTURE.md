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

---

## 1. 프로젝트 구조

```
ccxt-rust/
├── src/
│   ├── lib.rs              # 라이브러리 진입점 및 re-exports
│   ├── exchange.rs         # Exchange trait 정의
│   ├── ws_exchange.rs      # WsExchange trait 정의
│   ├── errors.rs           # CcxtError 타입 정의
│   │
│   ├── client/             # HTTP/WS 클라이언트
│   │   ├── mod.rs
│   │   ├── http.rs         # HTTP 클라이언트
│   │   └── websocket.rs    # WebSocket 클라이언트
│   │
│   ├── types/              # 공통 타입 정의
│   │   ├── mod.rs
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
│   │   ├── liquidation.rs  # Liquidation, OpenInterest
│   │   └── deposit.rs      # DepositAddress, Ledger
│   │
│   ├── exchanges/          # 거래소 구현체
│   │   ├── mod.rs
│   │   ├── foreign/        # 해외 거래소
│   │   │   ├── mod.rs
│   │   │   ├── binance.rs
│   │   │   ├── binance_ws.rs
│   │   │   ├── okx.rs
│   │   │   ├── bybit.rs
│   │   │   ├── gate.rs
│   │   │   ├── kucoin.rs
│   │   │   └── bitget.rs
│   │   │
│   │   └── korean/         # 국내 거래소
│   │       ├── mod.rs
│   │       ├── upbit.rs
│   │       ├── bithumb.rs
│   │       └── coinone.rs
│   │
│   └── utils/              # 유틸리티
│       ├── mod.rs
│       └── precise.rs      # 정밀 연산
│
├── tests/                  # 테스트
│   ├── unit/               # 단위 테스트
│   └── integration/        # 통합 테스트
│
├── examples/               # 예제 코드
│   ├── basic_usage.rs
│   └── websocket.rs
│
└── docs/                   # 문서
    ├── COMPARISON_ANALYSIS.md
    ├── IMPROVEMENT_ROADMAP.md
    └── ARCHITECTURE.md     # (이 문서)
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
│   exchange.rs   │  │ ws_exchange.rs  │  │    errors.rs    │
│ (Exchange trait)│  │(WsExchange trait)│  │  (CcxtError)   │
└─────────────────┘  └─────────────────┘  └─────────────────┘
          │                   │                   ▲
          │                   │                   │
          ▼                   ▼                   │
┌─────────────────────────────────────────────────┴───────────┐
│                     exchanges/                               │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │ Binance  │  │   OKX    │  │  Upbit   │  │   ...    │    │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘    │
└─────────────────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────┐
│                     client/                                  │
│  ┌──────────────────┐      ┌──────────────────┐            │
│  │   HttpClient     │      │  WebSocketClient │            │
│  │  (reqwest 기반)   │      │ (tungstenite 기반) │            │
│  └──────────────────┘      └──────────────────┘            │
└─────────────────────────────────────────────────────────────┘
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
    ├─► 인증 서명 (HMAC-SHA256/512, JWT, ...)
    │
    ▼
HttpClient
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

### 3.1 Exchange Trait

```rust
#[async_trait]
pub trait Exchange: Send + Sync {
    // === 기본 정보 ===
    fn id(&self) -> ExchangeId;
    fn name(&self) -> &'static str;
    fn has(&self) -> ExchangeCapabilities;

    // === Public API ===
    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>>;
    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>>;
    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, Currency>>;
    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker>;
    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>>;
    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook>;
    async fn fetch_trades(&self, symbol: &str, ...) -> CcxtResult<Vec<Trade>>;
    async fn fetch_ohlcv(&self, symbol: &str, ...) -> CcxtResult<Vec<OHLCV>>;

    // === Private Trading API ===
    async fn fetch_balance(&self) -> CcxtResult<Balances>;
    async fn create_order(&self, ...) -> CcxtResult<Order>;
    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;
    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;
    async fn fetch_open_orders(&self, ...) -> CcxtResult<Vec<Order>>;
    async fn fetch_closed_orders(&self, ...) -> CcxtResult<Vec<Order>>;
    async fn fetch_my_trades(&self, ...) -> CcxtResult<Vec<Trade>>;

    // === Derivatives API ===
    async fn fetch_position(&self, symbol: &str) -> CcxtResult<Position>;
    async fn fetch_positions(&self, ...) -> CcxtResult<Vec<Position>>;
    async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage>;
    async fn set_margin_mode(&self, ...) -> CcxtResult<MarginModeInfo>;
    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate>;
    // ... 더 많은 메서드
}
```

### 3.2 기본 구현 패턴

```rust
// 대부분의 메서드는 기본 구현 제공
async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
    Err(CcxtError::NotSupported {
        message: "fetch_ticker not supported".into(),
    })
}

// 거래소별로 오버라이드
impl Exchange for Binance {
    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_symbol = self.convert_symbol(symbol)?;
        let response: BinanceTicker = self.http
            .get("/api/v3/ticker/24hr", Some(&[("symbol", &market_symbol)]), None)
            .await?;
        self.parse_ticker(response, symbol)
    }
}
```

### 3.3 WsExchange Trait

```rust
#[async_trait]
pub trait WsExchange: Exchange {
    // === Public Streams ===
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<UnboundedReceiver<WsMessage>>;

    // === Private Streams ===
    async fn watch_balance(&self) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<UnboundedReceiver<WsMessage>>;
    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<UnboundedReceiver<WsMessage>>;

    // === Connection Management ===
    async fn ws_connect(&mut self) -> CcxtResult<()>;
    async fn ws_close(&mut self) -> CcxtResult<()>;
    async fn ws_is_connected(&self) -> bool;
    async fn ws_authenticate(&mut self) -> CcxtResult<()>;
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
    pub precision: Precision, // 가격/수량 정밀도
    pub limits: Limits,       // 최소/최대 한도
    // ... 더 많은 필드
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
    // ... 더 많은 필드
}

// Position - 포지션 정보 (선물/마진)
pub struct Position {
    pub symbol: String,
    pub side: PositionSide,    // long, short
    pub contracts: Decimal,    // 계약 수량
    pub entry_price: Decimal,  // 진입가
    pub mark_price: Option<Decimal>,
    pub liquidation_price: Option<Decimal>,
    pub unrealized_pnl: Option<Decimal>,
    pub leverage: Option<Decimal>,
    pub margin_mode: MarginMode, // cross, isolated
    // ... 더 많은 필드
}
```

### 4.2 Enum 설계

```rust
// 주문 타입 - 확장 가능한 설계
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
    StopMarket,
    StopLimit,
    TakeProfitMarket,
    TakeProfitLimit,
    TrailingStop,
    PostOnly,
}

// 타임프레임 - 문자열 변환 지원
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeframe {
    Minute1,
    Minute3,
    Minute5,
    Minute15,
    Minute30,
    Hour1,
    Hour2,
    Hour4,
    Hour6,
    Hour8,
    Hour12,
    Day1,
    Day3,
    Week1,
    Month1,
}

impl Timeframe {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Minute1 => "1m",
            Self::Hour1 => "1h",
            Self::Day1 => "1d",
            // ...
        }
    }

    pub fn to_milliseconds(&self) -> i64 {
        match self {
            Self::Minute1 => 60_000,
            Self::Hour1 => 3_600_000,
            Self::Day1 => 86_400_000,
            // ...
        }
    }
}
```

### 4.3 Builder 패턴

```rust
// Balances - Builder 패턴
pub struct Balances {
    balances: HashMap<String, Balance>,
    info: Value,
}

impl Balances {
    pub fn new() -> Self {
        Self {
            balances: HashMap::new(),
            info: Value::Null,
        }
    }

    pub fn add(mut self, currency: &str, balance: Balance) -> Self {
        self.balances.insert(currency.to_string(), balance);
        self
    }

    pub fn with_info(mut self, info: Value) -> Self {
        self.info = info;
        self
    }

    pub fn get(&self, currency: &str) -> Option<&Balance> {
        self.balances.get(currency)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Balance)> {
        self.balances.iter()
    }
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

    #[error("Permission denied: {message}")]
    PermissionDenied { message: String },

    #[error("Not supported: {message}")]
    NotSupported { message: String },

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

    #[error("Request timeout: {message}")]
    RequestTimeout { message: String },

    // ... 더 많은 에러 타입
}
```

### 5.2 에러 헬퍼 메서드

```rust
impl CcxtError {
    /// 에러 코드 반환
    pub fn code(&self) -> &'static str {
        match self {
            Self::ExchangeError { .. } => "EXCHANGE_ERROR",
            Self::AuthenticationError { .. } => "AUTH_ERROR",
            Self::RateLimitExceeded { .. } => "RATE_LIMIT",
            // ...
        }
    }

    /// 재시도 가능 여부
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::NetworkError { .. }
                | Self::RateLimitExceeded { .. }
                | Self::RequestTimeout { .. }
                | Self::ExchangeNotAvailable { .. }
        )
    }

    /// 인증 에러 여부
    pub fn is_auth_error(&self) -> bool {
        matches!(
            self,
            Self::AuthenticationError { .. }
                | Self::PermissionDenied { .. }
                | Self::InvalidNonce { .. }
        )
    }
}
```

### 5.3 거래소별 에러 매핑

```rust
// 거래소별 에러 코드 → CcxtError 변환
impl Binance {
    fn map_error(&self, code: i64, message: &str) -> CcxtError {
        match code {
            -1000 => CcxtError::ExchangeError { message: message.into() },
            -1002 => CcxtError::AuthenticationError { message: message.into() },
            -1015 => CcxtError::RateLimitExceeded { message: message.into() },
            -2010 => CcxtError::InsufficientFunds { message: message.into() },
            -2011 => CcxtError::OrderNotFound { order_id: "unknown".into() },
            -2013 => CcxtError::OrderNotFound { order_id: message.into() },
            _ => CcxtError::ExchangeError { message: format!("[{}] {}", code, message) },
        }
    }
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
    pub fn new(base_url: &str, timeout_ms: Option<u64>) -> Self {
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(30_000));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_url: base_url.to_string(),
        }
    }

    /// GET 요청
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<&[(&str, &str)]>,
        headers: Option<HeaderMap>,
    ) -> CcxtResult<T> {
        // ...
    }

    /// POST 요청
    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<&str>,
        headers: Option<HeaderMap>,
    ) -> CcxtResult<T> {
        // ...
    }

    /// DELETE 요청
    pub async fn delete<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<&[(&str, &str)]>,
        headers: Option<HeaderMap>,
    ) -> CcxtResult<T> {
        // ...
    }
}
```

### 6.2 요청 흐름

```
1. 파라미터 직렬화
   ├─ Query string 생성 (GET, DELETE)
   └─ Body 생성 (POST)

2. 헤더 설정
   ├─ Content-Type: application/json
   ├─ 인증 헤더 (API Key, Signature, ...)
   └─ 커스텀 헤더

3. 요청 전송
   └─ reqwest::Client::send()

4. 응답 처리
   ├─ Status code 확인
   ├─ 에러 응답 파싱
   └─ 정상 응답 역직렬화
```

---

## 7. 인증 체계

### 7.1 인증 방식별 분류

| 방식 | 거래소 | 설명 |
|------|--------|------|
| HMAC-SHA256 | Binance, OKX, Gate.io | 표준 서명 |
| HMAC-SHA256 + Passphrase | Kucoin, Bitget | 추가 암호 |
| HMAC-SHA512 | Bithumb, Coinone | SHA512 해시 |
| JWT | Upbit | JSON Web Token |
| Ed25519 | 일부 거래소 | EdDSA 서명 |

### 7.2 HMAC-SHA256 구현 예시 (Binance)

```rust
impl Binance {
    fn sign(&self, params: &str) -> CcxtResult<String> {
        let api_secret = self.config.api_secret.as_ref()
            .ok_or(CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid API secret".into(),
            })?;

        mac.update(params.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }
}
```

### 7.3 JWT 구현 예시 (Upbit)

```rust
impl Upbit {
    fn create_jwt(&self, params: Option<&HashMap<String, String>>) -> CcxtResult<String> {
        let access_key = self.config.api_key.as_ref()
            .ok_or(CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let secret_key = self.config.api_secret.as_ref()
            .ok_or(CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        let mut claims = json!({
            "access_key": access_key,
            "nonce": uuid::Uuid::new_v4().to_string(),
        });

        // 쿼리 해시 추가 (파라미터 있는 경우)
        if let Some(p) = params {
            let query_string = self.build_query_string(p);
            let query_hash = sha512_hash(&query_string);
            claims["query_hash"] = json!(query_hash);
            claims["query_hash_alg"] = json!("SHA512");
        }

        let header = jsonwebtoken::Header::new(Algorithm::HS256);
        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_secret(secret_key.as_bytes()),
        )?;

        Ok(format!("Bearer {}", token))
    }
}
```

---

## 8. WebSocket 아키텍처

### 8.1 연결 관리

```rust
pub struct WsConnection {
    stream: WebSocketStream<TcpStream>,
    subscriptions: HashSet<String>,
    ping_interval: Duration,
    last_pong: Instant,
}

impl WsConnection {
    /// 연결 유지 (ping/pong)
    pub async fn keepalive(&mut self) -> CcxtResult<()> {
        if self.last_pong.elapsed() > self.ping_interval * 2 {
            return Err(CcxtError::NetworkError {
                message: "WebSocket connection timeout".into(),
            });
        }

        self.stream.send(Message::Ping(vec![])).await?;
        Ok(())
    }

    /// 구독 관리
    pub async fn subscribe(&mut self, channel: &str) -> CcxtResult<()> {
        if self.subscriptions.contains(channel) {
            return Ok(());
        }

        // 구독 메시지 전송
        let msg = self.build_subscribe_message(channel);
        self.stream.send(Message::Text(msg)).await?;
        self.subscriptions.insert(channel.to_string());

        Ok(())
    }
}
```

### 8.2 메시지 처리

```rust
pub enum WsMessage {
    Ticker(Ticker),
    OrderBook(OrderBook),
    Trade(Trade),
    OHLCV(OHLCV),
    Balance(Balances),
    Order(Order),
    Position(Position),
    Error(String),
}

impl BinanceWs {
    fn parse_message(&self, text: &str) -> CcxtResult<WsMessage> {
        let data: Value = serde_json::from_str(text)?;

        // 이벤트 타입에 따라 파싱
        match data.get("e").and_then(|v| v.as_str()) {
            Some("24hrTicker") => self.parse_ticker_message(&data),
            Some("depthUpdate") => self.parse_orderbook_message(&data),
            Some("trade") => self.parse_trade_message(&data),
            Some("kline") => self.parse_ohlcv_message(&data),
            Some("outboundAccountPosition") => self.parse_balance_message(&data),
            Some("executionReport") => self.parse_order_message(&data),
            _ => Err(CcxtError::BadResponse {
                message: "Unknown message type".into(),
            }),
        }
    }
}
```

### 8.3 재연결 전략

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

impl WsConnection {
    async fn reconnect_with_backoff(&mut self, config: &ReconnectConfig) -> CcxtResult<()> {
        let mut delay = config.initial_delay;
        let mut retries = 0;

        while retries < config.max_retries {
            match self.connect().await {
                Ok(_) => {
                    // 기존 구독 복원
                    for channel in self.subscriptions.clone() {
                        self.subscribe(&channel).await?;
                    }
                    return Ok(());
                }
                Err(_) => {
                    tokio::time::sleep(delay).await;
                    delay = (delay.mul_f64(config.backoff_factor)).min(config.max_delay);
                    retries += 1;
                }
            }
        }

        Err(CcxtError::NetworkError {
            message: "Max reconnection attempts exceeded".into(),
        })
    }
}
```

---

## 관련 문서

- [COMPARISON_ANALYSIS.md](./COMPARISON_ANALYSIS.md) - CCXT Reference와의 비교 분석
- [IMPROVEMENT_ROADMAP.md](./IMPROVEMENT_ROADMAP.md) - 개선 로드맵
