# CCXT-Rust 개선 로드맵

> 이 문서는 ccxt-rust 프로젝트의 단계별 개선 계획을 정의합니다.
>
> 관련 문서: [COMPARISON_ANALYSIS.md](./COMPARISON_ANALYSIS.md)

## 목차

1. [Phase 5: 핵심 API 확장](#phase-5-핵심-api-확장)
2. [Phase 6: WebSocket 확장](#phase-6-websocket-확장)
3. [Phase 7: 헬퍼 함수 구현](#phase-7-헬퍼-함수-구현)
4. [Phase 8: 고급 주문 타입](#phase-8-고급-주문-타입)
5. [Phase 9: 추가 거래소](#phase-9-추가-거래소)
6. [Phase 10: 옵션 및 기타](#phase-10-옵션-및-기타)

---

## Phase 5: 핵심 API 확장

### 5.1 주문 관리 고급 API (우선순위: 🔴 높음)

#### 5.1.1 Exchange Trait 확장

```rust
// src/exchange.rs에 추가
#[async_trait]
pub trait Exchange: Send + Sync {
    // 기존 메서드들...

    // === 새로 추가할 메서드 ===

    /// 주문 수정
    async fn edit_order(
        &self,
        id: &str,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Option<Decimal>,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        Err(CcxtError::NotSupported {
            message: "edit_order not supported".into(),
        })
    }

    /// 모든 주문 취소
    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            message: "cancel_all_orders not supported".into(),
        })
    }

    /// 복수 주문 생성
    async fn create_orders(
        &self,
        orders: Vec<OrderRequest>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            message: "create_orders not supported".into(),
        })
    }
}
```

#### 5.1.2 새로운 타입 정의

```rust
// src/types/order.rs에 추가

#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub order_type: OrderType,
    pub side: OrderSide,
    pub amount: Decimal,
    pub price: Option<Decimal>,
    pub params: Option<HashMap<String, Value>>,
}

#[derive(Debug, Clone)]
pub struct StopOrderParams {
    pub trigger_price: Decimal,
    pub trigger_type: TriggerType, // mark, last, index
    pub reduce_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerType {
    Mark,
    Last,
    Index,
}
```

#### 5.1.3 구현 작업 목록

| 메서드 | 거래소 | 난이도 | 예상 작업 |
|--------|--------|--------|-----------|
| `edit_order` | Binance, OKX, Bybit | 중 | API 매핑 + 응답 파싱 |
| `cancel_all_orders` | 전체 | 하 | 기존 cancel 로직 확장 |
| `create_orders` | Binance, OKX | 중 | 배치 API 구현 |
| `fetch_order_trades` | 전체 | 하 | my_trades 필터링 |

### 5.2 수수료 API (우선순위: 🔴 높음)

#### 5.2.1 Exchange Trait 확장

```rust
/// 거래 수수료 전체 조회
async fn fetch_trading_fees(&self) -> CcxtResult<HashMap<String, TradingFee>> {
    Err(CcxtError::NotSupported {
        message: "fetch_trading_fees not supported".into(),
    })
}

/// 개별 심볼 수수료 조회
async fn fetch_trading_fee(&self, symbol: &str) -> CcxtResult<TradingFee> {
    Err(CcxtError::NotSupported {
        message: "fetch_trading_fee not supported".into(),
    })
}

/// 입출금 수수료 조회
async fn fetch_deposit_withdraw_fees(
    &self,
    codes: Option<&[&str]>,
) -> CcxtResult<HashMap<String, DepositWithdrawFee>> {
    Err(CcxtError::NotSupported {
        message: "fetch_deposit_withdraw_fees not supported".into(),
    })
}
```

#### 5.2.2 새로운 타입 정의

```rust
// src/types/fee.rs (신규 파일)

#[derive(Debug, Clone, Default)]
pub struct TradingFee {
    pub symbol: String,
    pub maker: Decimal,
    pub taker: Decimal,
    pub percentage: bool,
    pub tier_based: bool,
    pub info: Value,
}

#[derive(Debug, Clone, Default)]
pub struct DepositWithdrawFee {
    pub currency: String,
    pub deposit: Option<FeeInfo>,
    pub withdraw: Option<FeeInfo>,
    pub networks: HashMap<String, NetworkFee>,
    pub info: Value,
}

#[derive(Debug, Clone, Default)]
pub struct FeeInfo {
    pub fee: Option<Decimal>,
    pub percentage: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkFee {
    pub network: String,
    pub deposit: Option<FeeInfo>,
    pub withdraw: Option<FeeInfo>,
}
```

### 5.3 시장 데이터 API (우선순위: 🟡 중간)

```rust
/// 서버 시간 조회
async fn fetch_time(&self) -> CcxtResult<i64> {
    Err(CcxtError::NotSupported {
        message: "fetch_time not supported".into(),
    })
}

/// 거래소 상태 조회
async fn fetch_status(&self) -> CcxtResult<ExchangeStatus> {
    Err(CcxtError::NotSupported {
        message: "fetch_status not supported".into(),
    })
}

/// L3 호가창 조회
async fn fetch_l3_order_book(
    &self,
    symbol: &str,
    limit: Option<u32>,
) -> CcxtResult<OrderBook> {
    Err(CcxtError::NotSupported {
        message: "fetch_l3_order_book not supported".into(),
    })
}

/// 최우선 호가 조회
async fn fetch_bids_asks(
    &self,
    symbols: Option<&[&str]>,
) -> CcxtResult<HashMap<String, BidAsk>> {
    Err(CcxtError::NotSupported {
        message: "fetch_bids_asks not supported".into(),
    })
}
```

---

## Phase 6: WebSocket 확장

### 6.1 구현 우선순위

| 순위 | 거래소 | 이유 | 난이도 |
|------|--------|------|--------|
| 1 | OKX | 거래량 높음, 다양한 스트림 | 중 |
| 2 | Bybit | 선물 인기, 깔끔한 API | 중 |
| 3 | Upbit | 국내 1위, JWT WS 인증 | 중상 |
| 4 | Gate.io | spot/futures 구분 | 중 |
| 5 | Kucoin | 토큰 기반 연결 | 상 |
| 6 | Bitget | 표준 WS | 중 |

### 6.2 OKX WebSocket 구현 예시

```rust
// src/exchanges/foreign/okx_ws.rs (신규 파일)

use tokio_tungstenite::{connect_async, WebSocketStream};
use futures_util::{StreamExt, SinkExt};

pub struct OkxWs {
    config: ExchangeConfig,
    ws_stream: Option<WebSocketStream<...>>,
    subscriptions: HashSet<String>,
}

impl OkxWs {
    const WS_PUBLIC_URL: &'static str = "wss://ws.okx.com:8443/ws/v5/public";
    const WS_PRIVATE_URL: &'static str = "wss://ws.okx.com:8443/ws/v5/private";

    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            ws_stream: None,
            subscriptions: HashSet::new(),
        }
    }

    async fn subscribe(&mut self, channel: &str, inst_id: &str) -> CcxtResult<()> {
        let msg = json!({
            "op": "subscribe",
            "args": [{
                "channel": channel,
                "instId": inst_id
            }]
        });
        // ...
    }
}

#[async_trait]
impl WsExchange for OkxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<UnboundedReceiver<WsMessage>> {
        self.subscribe("tickers", symbol).await?;
        // ...
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<UnboundedReceiver<WsMessage>> {
        let channel = match limit {
            Some(5) => "books5",
            Some(50) => "books50-l2-tbt",
            _ => "books",
        };
        self.subscribe(channel, symbol).await?;
        // ...
    }
}
```

### 6.3 WsExchange Trait 확장

```rust
// 추가할 WebSocket 메서드

/// 포지션 실시간 구독
async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<UnboundedReceiver<WsMessage>> {
    Err(CcxtError::NotSupported {
        message: "watch_positions not supported".into(),
    })
}

/// 펀딩비 실시간 구독
async fn watch_funding_rate(&self, symbol: &str) -> CcxtResult<UnboundedReceiver<WsMessage>> {
    Err(CcxtError::NotSupported {
        message: "watch_funding_rate not supported".into(),
    })
}

/// 청산 실시간 구독
async fn watch_liquidations(&self, symbol: &str) -> CcxtResult<UnboundedReceiver<WsMessage>> {
    Err(CcxtError::NotSupported {
        message: "watch_liquidations not supported".into(),
    })
}

/// 구독 해제
async fn unwatch(&self, subscription_id: &str) -> CcxtResult<()> {
    Err(CcxtError::NotSupported {
        message: "unwatch not supported".into(),
    })
}
```

---

## Phase 7: 헬퍼 함수 구현

### 7.1 Safe* 헬퍼 함수 모듈

```rust
// src/utils/safe.rs (신규 파일)

use serde_json::Value;
use rust_decimal::Decimal;
use std::str::FromStr;

/// 안전한 문자열 추출
pub fn safe_string(obj: &Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
}

/// 두 키 중 하나에서 문자열 추출
pub fn safe_string2(obj: &Value, key1: &str, key2: &str) -> Option<String> {
    safe_string(obj, key1).or_else(|| safe_string(obj, key2))
}

/// N개 키 중 하나에서 문자열 추출
pub fn safe_string_n(obj: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| safe_string(obj, k))
}

/// 소문자 문자열 추출
pub fn safe_string_lower(obj: &Value, key: &str) -> Option<String> {
    safe_string(obj, key).map(|s| s.to_lowercase())
}

/// 대문자 문자열 추출
pub fn safe_string_upper(obj: &Value, key: &str) -> Option<String> {
    safe_string(obj, key).map(|s| s.to_uppercase())
}

/// 안전한 정수 추출
pub fn safe_integer(obj: &Value, key: &str) -> Option<i64> {
    obj.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// 두 키 중 하나에서 정수 추출
pub fn safe_integer2(obj: &Value, key1: &str, key2: &str) -> Option<i64> {
    safe_integer(obj, key1).or_else(|| safe_integer(obj, key2))
}

/// 안전한 실수 추출
pub fn safe_float(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// 안전한 Decimal 추출
pub fn safe_decimal(obj: &Value, key: &str) -> Option<Decimal> {
    obj.get(key).and_then(|v| match v {
        Value::String(s) => Decimal::from_str(s).ok(),
        Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    })
}

/// 두 키 중 하나에서 Decimal 추출
pub fn safe_decimal2(obj: &Value, key1: &str, key2: &str) -> Option<Decimal> {
    safe_decimal(obj, key1).or_else(|| safe_decimal(obj, key2))
}

/// 안전한 타임스탬프 추출 (밀리초)
pub fn safe_timestamp(obj: &Value, key: &str) -> Option<i64> {
    safe_integer(obj, key).or_else(|| {
        safe_string(obj, key).and_then(|s| {
            // ISO 8601 파싱 시도
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.timestamp_millis())
                .ok()
        })
    })
}

/// 안전한 값 추출
pub fn safe_value(obj: &Value, key: &str) -> Option<&Value> {
    obj.get(key).filter(|v| !v.is_null())
}

/// 두 키 중 하나에서 값 추출
pub fn safe_value2<'a>(obj: &'a Value, key1: &str, key2: &str) -> Option<&'a Value> {
    safe_value(obj, key1).or_else(|| safe_value(obj, key2))
}
```

### 7.2 Parse* 헬퍼 함수 (각 거래소별)

```rust
// src/utils/parse.rs (신규 파일)

use crate::types::*;
use crate::utils::safe::*;

/// 범용 주문 파싱 (거래소별 커스터마이징 가능)
pub trait OrderParser {
    fn parse_order(&self, data: &Value, market: Option<&Market>) -> CcxtResult<Order>;
}

/// 범용 거래 파싱
pub trait TradeParser {
    fn parse_trade(&self, data: &Value, market: Option<&Market>) -> CcxtResult<Trade>;
}

/// 범용 티커 파싱
pub trait TickerParser {
    fn parse_ticker(&self, data: &Value, market: Option<&Market>) -> CcxtResult<Ticker>;
}

/// 범용 호가창 파싱
pub trait OrderBookParser {
    fn parse_order_book(&self, data: &Value, symbol: &str) -> CcxtResult<OrderBook>;
}

/// 기본 호가창 파싱 구현
pub fn parse_order_book_default(
    data: &Value,
    symbol: &str,
    bids_key: &str,
    asks_key: &str,
) -> CcxtResult<OrderBook> {
    let timestamp = safe_timestamp(data, "timestamp")
        .or_else(|| safe_timestamp(data, "ts"))
        .or_else(|| safe_timestamp(data, "T"));

    let bids = parse_order_book_side(data, bids_key)?;
    let asks = parse_order_book_side(data, asks_key)?;

    Ok(OrderBook {
        symbol: symbol.to_string(),
        timestamp,
        datetime: timestamp.map(|ts| {
            chrono::DateTime::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
        }).flatten(),
        bids,
        asks,
        nonce: safe_integer(data, "nonce"),
    })
}

fn parse_order_book_side(data: &Value, key: &str) -> CcxtResult<Vec<OrderBookEntry>> {
    let entries = data.get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| CcxtError::BadResponse {
            message: format!("Missing {} in order book", key),
        })?;

    entries.iter().map(|entry| {
        let arr = entry.as_array().ok_or_else(|| CcxtError::BadResponse {
            message: "Invalid order book entry format".into(),
        })?;

        let price = parse_decimal_from_value(&arr[0])?;
        let amount = parse_decimal_from_value(&arr[1])?;

        Ok(OrderBookEntry { price, amount })
    }).collect()
}

fn parse_decimal_from_value(v: &Value) -> CcxtResult<Decimal> {
    match v {
        Value::String(s) => Decimal::from_str(s).map_err(|_| CcxtError::BadResponse {
            message: format!("Invalid decimal: {}", s),
        }),
        Value::Number(n) => Decimal::from_str(&n.to_string()).map_err(|_| CcxtError::BadResponse {
            message: format!("Invalid decimal: {}", n),
        }),
        _ => Err(CcxtError::BadResponse {
            message: "Expected string or number for decimal".into(),
        }),
    }
}
```

### 7.3 Precision 헬퍼 함수 확장

```rust
// src/utils/precise.rs 확장

impl Precise {
    /// 심볼에 맞는 수량 정밀도 적용
    pub fn amount_to_precision(
        amount: &Decimal,
        precision: &Precision,
    ) -> Decimal {
        match precision.amount {
            Some(p) => amount.round_dp(p as u32),
            None => *amount,
        }
    }

    /// 심볼에 맞는 가격 정밀도 적용
    pub fn price_to_precision(
        price: &Decimal,
        precision: &Precision,
    ) -> Decimal {
        match precision.price {
            Some(p) => price.round_dp(p as u32),
            None => *price,
        }
    }

    /// 비용 정밀도 적용
    pub fn cost_to_precision(
        cost: &Decimal,
        precision: &Precision,
    ) -> Decimal {
        // 기본적으로 8자리
        cost.round_dp(8)
    }

    /// 반올림 모드 지정
    pub fn decimal_to_precision(
        value: &Decimal,
        rounding: RoundingMode,
        precision: u32,
    ) -> Decimal {
        match rounding {
            RoundingMode::Round => value.round_dp(precision),
            RoundingMode::Truncate => value.trunc_with_scale(precision),
            RoundingMode::Ceiling => {
                let factor = Decimal::new(10i64.pow(precision), 0);
                (*value * factor).ceil() / factor
            }
            RoundingMode::Floor => {
                let factor = Decimal::new(10i64.pow(precision), 0);
                (*value * factor).floor() / factor
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RoundingMode {
    Round,
    Truncate,
    Ceiling,
    Floor,
}
```

---

## Phase 8: 고급 주문 타입

### 8.1 스탑 주문 구현

```rust
// Exchange trait 확장

/// 스탑 주문 생성
async fn create_stop_order(
    &self,
    symbol: &str,
    order_type: OrderType,
    side: OrderSide,
    amount: Decimal,
    price: Option<Decimal>,
    stop_price: Decimal,
    params: Option<HashMap<String, Value>>,
) -> CcxtResult<Order> {
    Err(CcxtError::NotSupported {
        message: "create_stop_order not supported".into(),
    })
}

/// 스탑 리밋 주문
async fn create_stop_limit_order(
    &self,
    symbol: &str,
    side: OrderSide,
    amount: Decimal,
    price: Decimal,
    stop_price: Decimal,
    params: Option<HashMap<String, Value>>,
) -> CcxtResult<Order> {
    self.create_stop_order(
        symbol,
        OrderType::Limit,
        side,
        amount,
        Some(price),
        stop_price,
        params,
    ).await
}

/// 스탑 마켓 주문
async fn create_stop_market_order(
    &self,
    symbol: &str,
    side: OrderSide,
    amount: Decimal,
    stop_price: Decimal,
    params: Option<HashMap<String, Value>>,
) -> CcxtResult<Order> {
    self.create_stop_order(
        symbol,
        OrderType::Market,
        side,
        amount,
        None,
        stop_price,
        params,
    ).await
}
```

### 8.2 익절/손절 주문

```rust
/// Take Profit 주문
async fn create_take_profit_order(
    &self,
    symbol: &str,
    order_type: OrderType,
    side: OrderSide,
    amount: Decimal,
    price: Option<Decimal>,
    take_profit_price: Decimal,
    params: Option<HashMap<String, Value>>,
) -> CcxtResult<Order> {
    Err(CcxtError::NotSupported {
        message: "create_take_profit_order not supported".into(),
    })
}

/// Stop Loss 주문
async fn create_stop_loss_order(
    &self,
    symbol: &str,
    order_type: OrderType,
    side: OrderSide,
    amount: Decimal,
    price: Option<Decimal>,
    stop_loss_price: Decimal,
    params: Option<HashMap<String, Value>>,
) -> CcxtResult<Order> {
    Err(CcxtError::NotSupported {
        message: "create_stop_loss_order not supported".into(),
    })
}

/// TP/SL 동시 설정 주문
async fn create_order_with_take_profit_and_stop_loss(
    &self,
    symbol: &str,
    order_type: OrderType,
    side: OrderSide,
    amount: Decimal,
    price: Option<Decimal>,
    take_profit_price: Option<Decimal>,
    stop_loss_price: Option<Decimal>,
    params: Option<HashMap<String, Value>>,
) -> CcxtResult<Order> {
    Err(CcxtError::NotSupported {
        message: "create_order_with_take_profit_and_stop_loss not supported".into(),
    })
}
```

### 8.3 특수 주문 타입

```rust
/// Post-only 주문 (Maker only)
async fn create_post_only_order(
    &self,
    symbol: &str,
    side: OrderSide,
    amount: Decimal,
    price: Decimal,
    params: Option<HashMap<String, Value>>,
) -> CcxtResult<Order> {
    let mut p = params.unwrap_or_default();
    p.insert("postOnly".into(), Value::Bool(true));
    self.create_limit_order(symbol, side, amount, price).await
}

/// Reduce-only 주문 (포지션 감소만)
async fn create_reduce_only_order(
    &self,
    symbol: &str,
    order_type: OrderType,
    side: OrderSide,
    amount: Decimal,
    price: Option<Decimal>,
    params: Option<HashMap<String, Value>>,
) -> CcxtResult<Order> {
    let mut p = params.unwrap_or_default();
    p.insert("reduceOnly".into(), Value::Bool(true));
    self.create_order(symbol, order_type, side, amount, price).await
}
```

---

## Phase 9: 추가 거래소

### 9.1 구현 우선순위

| 순위 | 거래소 | 일일 거래량 | 난이도 | 비고 |
|------|--------|-------------|--------|------|
| 1 | Kraken | 상위 10 | 중 | 미국 규제 |
| 2 | Huobi/HTX | 상위 10 | 중 | 아시아 인기 |
| 3 | MEXC | 상위 15 | 중 | 알트코인 다양 |
| 4 | Phemex | 상위 20 | 중 | 파생상품 |
| 5 | dYdX | 상위 DEX | 상 | Web3 통합 필요 |
| 6 | Crypto.com | 상위 15 | 중 | 모바일 인기 |
| 7 | KuCoin | - | 완료 | ✅ |
| 8 | Bitget | - | 완료 | ✅ |

### 9.2 Kraken 구현 예시

```rust
// src/exchanges/foreign/kraken.rs (신규)

pub struct Kraken {
    config: ExchangeConfig,
    http: HttpClient,
    markets: Option<HashMap<String, Market>>,
}

impl Kraken {
    const BASE_URL: &'static str = "https://api.kraken.com";

    pub fn new(config: ExchangeConfig) -> Self {
        let http = HttpClient::new(Self::BASE_URL, config.timeout_ms);
        Self {
            config,
            http,
            markets: None,
        }
    }

    fn sign(&self, path: &str, nonce: u64, body: &str) -> CcxtResult<String> {
        let api_secret = self.config.api_secret.as_ref()
            .ok_or(CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        // Kraken: SHA256(nonce + body) -> HMAC-SHA512(path + sha256, base64_decode(secret))
        let sha256_data = format!("{}{}", nonce, body);
        let sha256_hash = Sha256::digest(sha256_data.as_bytes());

        let sign_data = [path.as_bytes(), &sha256_hash[..]].concat();
        let secret_bytes = BASE64.decode(api_secret)?;

        let mut mac = HmacSha512::new_from_slice(&secret_bytes)?;
        mac.update(&sign_data);

        Ok(BASE64.encode(mac.finalize().into_bytes()))
    }
}

#[async_trait]
impl Exchange for Kraken {
    fn id(&self) -> ExchangeId {
        ExchangeId::Kraken
    }

    fn name(&self) -> &'static str {
        "Kraken"
    }

    // ... 구현 ...
}
```

### 9.3 국내 거래소 추가

| 거래소 | 상태 | 비고 |
|--------|------|------|
| Upbit | ✅ 완료 | JWT 인증 |
| Bithumb | ✅ 완료 | HMAC-SHA512 |
| Coinone | ✅ 완료 | HMAC-SHA512 |
| Korbit | ❌ 미구현 | OAuth 2.0 필요 |
| Gopax | ❌ 미구현 | 거래량 낮음 |

---

## Phase 10: 옵션 및 기타

### 10.1 옵션 거래 타입

```rust
// src/types/options.rs (신규 파일)

#[derive(Debug, Clone, Default)]
pub struct Greeks {
    pub symbol: String,
    pub timestamp: Option<i64>,
    pub datetime: Option<String>,
    pub delta: Option<Decimal>,
    pub gamma: Option<Decimal>,
    pub theta: Option<Decimal>,
    pub vega: Option<Decimal>,
    pub rho: Option<Decimal>,
    pub bid_iv: Option<Decimal>,  // Bid Implied Volatility
    pub ask_iv: Option<Decimal>,  // Ask Implied Volatility
    pub mark_iv: Option<Decimal>, // Mark Implied Volatility
    pub bid_price: Option<Decimal>,
    pub ask_price: Option<Decimal>,
    pub mark_price: Option<Decimal>,
    pub last_price: Option<Decimal>,
    pub underlying_price: Option<Decimal>,
    pub info: Value,
}

#[derive(Debug, Clone, Default)]
pub struct OptionContract {
    pub symbol: String,
    pub currency: String,
    pub base: String,
    pub quote: String,
    pub strike: Decimal,
    pub expiry: i64,
    pub expiry_datetime: String,
    pub option_type: OptionType,
    pub info: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionType {
    Call,
    Put,
}
```

### 10.2 옵션 API 메서드

```rust
/// Greeks 조회
async fn fetch_greeks(&self, symbol: &str) -> CcxtResult<Greeks> {
    Err(CcxtError::NotSupported {
        message: "fetch_greeks not supported".into(),
    })
}

/// 전체 Greeks 조회
async fn fetch_all_greeks(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Greeks>> {
    Err(CcxtError::NotSupported {
        message: "fetch_all_greeks not supported".into(),
    })
}

/// 옵션 체인 조회
async fn fetch_option_chain(
    &self,
    currency: &str,
    expiry: Option<i64>,
) -> CcxtResult<HashMap<String, OptionContract>> {
    Err(CcxtError::NotSupported {
        message: "fetch_option_chain not supported".into(),
    })
}

/// 개별 옵션 조회
async fn fetch_option(&self, symbol: &str) -> CcxtResult<OptionContract> {
    Err(CcxtError::NotSupported {
        message: "fetch_option not supported".into(),
    })
}
```

### 10.3 변환 API

```rust
/// 변환 견적 조회
async fn fetch_convert_quote(
    &self,
    from_code: &str,
    to_code: &str,
    amount: Option<Decimal>,
) -> CcxtResult<ConvertQuote> {
    Err(CcxtError::NotSupported {
        message: "fetch_convert_quote not supported".into(),
    })
}

/// 변환 가능 통화 목록
async fn fetch_convert_currencies(&self) -> CcxtResult<HashMap<String, Currency>> {
    Err(CcxtError::NotSupported {
        message: "fetch_convert_currencies not supported".into(),
    })
}

/// 변환 실행
async fn convert(
    &self,
    from_code: &str,
    to_code: &str,
    amount: Decimal,
    quote_id: Option<&str>,
) -> CcxtResult<Conversion> {
    Err(CcxtError::NotSupported {
        message: "convert not supported".into(),
    })
}
```

---

## 일정 요약

| Phase | 내용 | 예상 범위 |
|-------|------|-----------|
| **Phase 5** | 핵심 API 확장 (수수료, 주문관리) | 15-20 메서드 |
| **Phase 6** | WebSocket 확장 (OKX, Bybit 등) | 5-6 거래소 |
| **Phase 7** | 헬퍼 함수 (safe*, parse*) | 30-40 함수 |
| **Phase 8** | 고급 주문 (스탑, TP/SL) | 10-15 메서드 |
| **Phase 9** | 추가 거래소 (Kraken 등) | 5-10 거래소 |
| **Phase 10** | 옵션 및 기타 | 선택적 |

---

## 관련 문서

- [COMPARISON_ANALYSIS.md](./COMPARISON_ANALYSIS.md) - CCXT Reference와의 상세 비교
- [ARCHITECTURE.md](./ARCHITECTURE.md) - 프로젝트 아키텍처 (예정)
- [API_REFERENCE.md](./API_REFERENCE.md) - API 레퍼런스 (예정)
