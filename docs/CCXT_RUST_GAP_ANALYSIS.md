# CCXT-Rust Gap Analysis

**비교 기준**: ccxt-reference (TypeScript/Python 원본) vs ccxt-rust (Rust 포팅)

**작성일**: 2025년 12월 20일
**최종 업데이트**: 2025년 12월 28일 (Phase 15 완료 반영)

---

## 목차

1. [요약](#1-요약)
2. [거래소 구현 현황](#2-거래소-구현-현황)
3. [Exchange Trait 메서드 Gap](#3-exchange-trait-메서드-gap)
4. [타입 시스템 Gap](#4-타입-시스템-gap)
5. [유틸리티 함수 Gap](#5-유틸리티-함수-gap)
6. [인프라스트럭처 Gap](#6-인프라스트럭처-gap)
7. [WebSocket 기능 Gap](#7-websocket-기능-gap)
8. [우선순위별 작업 목록](#8-우선순위별-작업-목록)

---

## 1. 요약

| 항목 | CCXT Reference | CCXT-Rust | 커버리지 |
|------|----------------|-----------|----------|
| **거래소 수** | 110+ | 19 | 17% |
| **Exchange 메서드** | 100+ | 70+ | ~70% |
| **WebSocket 메서드** | 30+ | 26 | ~87% |
| **타입 정의** | 60+ | 45+ | ~75% |
| **에러 타입** | 30+ | 30+ | 100% |
| **유틸리티 함수** | 50+ | 25+ | ~50% |

### 핵심 Gap 요약

1. **거래소**: 91개 거래소 미구현
2. **메서드**: ~~주문 편집, 계정 전송~~ ✅ 완료 / 마진 대출 등 미구현
3. **타입**: Greeks, OptionChain, Conversion 등 미구현
4. **유틸리티**: RSA, TOTP, 고급 암호화 미구현

### 최근 완료된 작업 (Phase 11-15)

- ✅ **Phase 11**: 선물 기능 확장 (Bitget, Kucoin, MEXC)
- ✅ **Phase 12**: 고급 주문 기능 (`edit_order`, `create_orders`, `cancel_all_orders`)
- ✅ **Phase 13**: 고급 시장 데이터 (`fetch_mark_price`, `fetch_mark_prices`, `fetch_mark_ohlcv`, `fetch_index_ohlcv`)
- ✅ **Phase 14**: 추가 계정 기능 (`transfer`, `add_margin`, `reduce_margin`, `set_position_mode`, `withdraw`, `fetch_deposit_address`)
- ✅ **Phase 15**: 마진 대출/상환 (`borrow_cross_margin`, `repay_cross_margin`, `fetch_cross_borrow_rate` - Binance, OKX, Bybit)

---

## 2. 거래소 구현 현황

### 2.1 구현 완료 (19개)

#### 한국 거래소 (4개)
| 거래소 | REST | WebSocket | 선물 |
|--------|------|-----------|------|
| Upbit | ✅ | ✅ | ❌ |
| Bithumb | ✅ | ✅ | ❌ |
| Coinone | ✅ | ✅ | ❌ |
| Korbit | ✅ | ✅ | ❌ |

#### 해외 거래소 (15개)
| 거래소 | REST | WebSocket | 선물 |
|--------|------|-----------|------|
| Binance | ✅ | ✅ | ✅ (별도 모듈) |
| Binance Futures | ✅ | ✅ | ✅ |
| OKX | ✅ | ✅ | ✅ |
| Bybit | ✅ | ✅ | ✅ |
| Gate | ✅ | ✅ | ✅ |
| KuCoin | ✅ | ✅ | ✅ |
| Bitget | ✅ | ✅ | ✅ |
| Coinbase | ✅ | ✅ | ❌ |
| Kraken | ✅ | ✅ | ❌ |
| HTX | ✅ | ✅ | ❌ |
| MEXC | ✅ | ✅ | ❌ |
| Bitmart | ✅ | ✅ | ❌ |
| Coinex | ✅ | ✅ | ❌ |
| Phemex | ✅ | ✅ | ❌ |
| BingX | ✅ | ✅ | ❌ |

### 2.2 미구현 거래소 (91개)

#### 우선순위 높음 - Certified Exchanges
```
- Hyperliquid (DEX)
- BitMEX
- Crypto.com
- HashKey
- WOO X / WOOFI PRO
- Deribit (옵션)
```

#### 우선순위 중간 - 주요 거래소
```
- Bitfinex
- Gemini
- Poloniex
- Huobi Global
- LBank
- AscendEX
- Bitstamp
- BitFlyer
- Bitrue
- WhiteBit
- XT
- ProBit
```

#### 기타 거래소 (74개)
```
alpaca, apex, arkham, backpack, bequant, bigone, bit2c,
bitbank, bitbns, bitopro, bitso, bitteam, bittrade,
bitvavo, blockchaincom, blofin, btcalpha, btcbox,
btcmarkets, btcturk, bullish, cex, coincatch, coincheck,
coinmate, coinmetro, coinsph, coinspot, cryptomus,
deepcoin, defx, delta, derive, digifinex, dydx, exmo,
fmfwio, foxbit, hibachi, hitbtc, hollaex,
independentreserve, indodax, krakenfutures, latoken,
luno, mercado, modetrade, myokx, ndax, novadax,
oceanex, okxus, onetrading, oxfun, p2b, paradex,
paymium, probit, timex, tokocrypto, toobit,
wavesexchange, yobit, zaif, zebpay, zonda
```

---

## 3. Exchange Trait 메서드 Gap

### 3.1 미구현 메서드

#### Trading 관련
| 메서드 | 설명 | 우선순위 | 상태 |
|--------|------|----------|------|
| `edit_order()` | 주문 수정 | 🔴 높음 | ✅ 완료 (Binance, OKX) |
| `create_orders()` | 다중 주문 생성 | 🟡 중간 | ✅ 완료 (Binance, OKX) |
| `cancel_all_orders()` | 모든 주문 취소 | 🟡 중간 | ✅ 완료 (Binance, OKX) |
| `cancel_orders_for_symbols()` | 심볼별 주문 취소 | 🟢 낮음 | ❌ 미구현 |
| `fetch_order_trades()` | 주문별 체결 내역 | 🟡 중간 | ❌ 미구현 |

#### Account/Wallet 관련
| 메서드 | 설명 | 우선순위 | 상태 |
|--------|------|----------|------|
| `transfer()` | 계정 간 전송 | 🔴 높음 | ✅ 완료 (Binance, OKX, Bybit 등) |
| `fetch_transfers()` | 전송 내역 조회 | 🟡 중간 | ❌ 미구현 |
| `fetch_ledger()` | 원장 내역 조회 | 🟡 중간 | ❌ 미구현 |
| `fetch_ledger_entry()` | 단일 원장 항목 | 🟢 낮음 | ❌ 미구현 |
| `withdraw()` | 출금 | 🔴 높음 | ✅ 완료 (HTX, MEXC 등) |
| `fetch_deposit_address()` | 입금 주소 조회 | 🔴 높음 | ✅ 완료 (HTX, MEXC, Kucoin, Gate) |

#### Margin 관련
| 메서드 | 설명 | 우선순위 | 상태 |
|--------|------|----------|------|
| `borrow_cross_margin()` | 교차 마진 대출 | 🔴 높음 | ✅ 완료 (Binance, OKX, Bybit) |
| `borrow_isolated_margin()` | 격리 마진 대출 | 🔴 높음 | ✅ 완료 (Binance) |
| `repay_cross_margin()` | 교차 마진 상환 | 🔴 높음 | ✅ 완료 (Binance, OKX, Bybit) |
| `repay_isolated_margin()` | 격리 마진 상환 | 🔴 높음 | ✅ 완료 (Binance) |
| `fetch_cross_borrow_rate()` | 교차 마진 이율 | 🟡 중간 | ✅ 완료 (Binance, OKX, Bybit) |
| `fetch_isolated_borrow_rate()` | 격리 마진 이율 | 🟡 중간 | ✅ 완료 (Binance) |
| `fetch_cross_borrow_rates()` | 교차 마진 이율 목록 | 🟡 중간 | ❌ 미구현 |
| `fetch_isolated_borrow_rates()` | 격리 마진 이율 목록 | 🟡 중간 | ❌ 미구현 |
| `add_margin()` | 마진 추가 | 🟡 중간 | ✅ 완료 (Binance, OKX, Bybit 등) |
| `reduce_margin()` | 마진 감소 | 🟡 중간 | ✅ 완료 (Binance, OKX, Bybit 등) |
| `set_margin()` | 마진 설정 | 🟡 중간 | ❌ 미구현 |
| `set_position_mode()` | 포지션 모드 설정 | 🟡 중간 | ✅ 완료 (Binance, OKX, Bybit 등) |
| `fetch_margin_adjustment_history()` | 마진 조정 이력 | 🟢 낮음 | ❌ 미구현 |

#### Derivatives 관련
| 메서드 | 설명 | 우선순위 | 상태 |
|--------|------|----------|------|
| `fetch_mark_price()` | 마크 가격 | 🔴 높음 | ✅ 완료 (OKX, Bybit, Bitget, Kucoin, MEXC) |
| `fetch_mark_prices()` | 마크 가격 목록 | 🔴 높음 | ✅ 완료 (OKX, Bybit, Bitget, Kucoin, MEXC) |
| `fetch_mark_ohlcv()` | 마크 가격 OHLCV | 🟡 중간 | ✅ 완료 (OKX, Bybit, Bitget) / NotSupported (Kucoin, MEXC) |
| `fetch_index_ohlcv()` | 인덱스 가격 OHLCV | 🟡 중간 | ✅ 완료 (OKX, Bybit, Bitget) / NotSupported (Kucoin, MEXC) |
| `fetch_greeks()` | 옵션 Greeks | 🟡 중간 | ❌ 미구현 |
| `fetch_option()` | 옵션 정보 | 🟡 중간 | ❌ 미구현 |
| `fetch_option_chain()` | 옵션 체인 | 🟡 중간 | ❌ 미구현 |
| `fetch_underlying_assets()` | 기초 자산 | 🟢 낮음 | ❌ 미구현 |
| `fetch_settlement_history()` | 결제 이력 | 🟢 낮음 | ❌ 미구현 |
| `fetch_volatility_history()` | 변동성 이력 | 🟢 낮음 | ❌ 미구현 |

#### Convert 관련
| 메서드 | 설명 | 우선순위 | 상태 |
|--------|------|----------|------|
| `fetch_convert_currencies()` | 변환 가능 통화 | 🟢 낮음 | ❌ 미구현 |
| `fetch_convert_quote()` | 변환 견적 | 🟢 낮음 | ❌ 미구현 |
| `create_convert_trade()` | 변환 거래 생성 | 🟢 낮음 | ❌ 미구현 |
| `fetch_convert_trade()` | 변환 거래 조회 | 🟢 낮음 | ❌ 미구현 |
| `fetch_convert_trade_history()` | 변환 거래 이력 | 🟢 낮음 | ❌ 미구현 |

### 3.2 부분 구현된 메서드

| 메서드 | 현재 상태 | 필요한 작업 |
|--------|-----------|-------------|
| `create_order()` | 기본 구현 | 고급 주문 타입 (OCO, Bracket) 지원 필요 |
| `fetch_positions()` | 선물 거래소만 | 모든 선물 거래소 확장 필요 |
| `fetch_liquidations()` | 일부만 구현 | REST API 없는 거래소는 WebSocket 구현 필요 |

---

## 4. 타입 시스템 Gap

### 4.1 미구현 타입

#### Options 관련
```rust
// 미구현
struct Greeks {
    delta: Option<Decimal>,
    gamma: Option<Decimal>,
    theta: Option<Decimal>,
    vega: Option<Decimal>,
    rho: Option<Decimal>,
}

struct OptionContract {
    symbol: String,
    underlying: String,
    strike: Decimal,
    option_type: OptionType, // Call, Put
    expiry: i64,
}

struct OptionChain {
    underlying: String,
    calls: Vec<OptionContract>,
    puts: Vec<OptionContract>,
}
```

#### Conversion 관련
```rust
// 미구현
struct ConvertQuote {
    from_currency: String,
    to_currency: String,
    from_amount: Decimal,
    to_amount: Decimal,
    rate: Decimal,
    inverse_rate: Decimal,
    expires: i64,
}

struct ConvertTrade {
    id: String,
    from_currency: String,
    to_currency: String,
    from_amount: Decimal,
    to_amount: Decimal,
    timestamp: i64,
    status: String,
}
```

#### Trading 확장
```rust
// 미구현
struct OrderBook2 {
    // Level 2 Order Book with order IDs
    bids: Vec<OrderBookEntryWithId>,
    asks: Vec<OrderBookEntryWithId>,
    nonce: Option<i64>,
}

struct OrderBookEntryWithId {
    price: Decimal,
    amount: Decimal,
    order_id: String,
}

struct StopLoss {
    trigger_price: Decimal,
    price: Option<Decimal>,
    type_: TriggerType,
}

struct TakeProfit {
    trigger_price: Decimal,
    price: Option<Decimal>,
    type_: TriggerType,
}
```

### 4.2 확장 필요 타입

| 타입 | 현재 상태 | 필요한 확장 |
|------|-----------|-------------|
| `Order` | 기본 필드 | `stop_loss`, `take_profit`, `reduce_only`, `post_only` 필드 추가 |
| `Position` | 기본 필드 | `hedged`, `stop_loss`, `take_profit` 필드 추가 |
| `Market` | 기본 필드 | `option` 관련 필드 추가 |
| `Ticker` | 기본 필드 | `percentage`, `average`, `previous_close` 필드 추가 |

---

## 5. 유틸리티 함수 Gap

### 5.1 암호화 관련

| 함수 | 설명 | 우선순위 |
|------|------|----------|
| `hmac_sha384()` | HMAC-SHA384 서명 | 🔴 높음 |
| `hmac_sha512()` | HMAC-SHA512 서명 | 🔴 높음 |
| `rsa_sign()` | RSA 서명 | 🔴 높음 |
| `ecdsa_sign()` | ECDSA 서명 | 🟡 중간 |
| `ed25519_sign()` | Ed25519 서명 | 🟡 중간 |
| `jwt_encode()` | JWT 토큰 생성 | 🔴 높음 |
| `jwt_decode()` | JWT 토큰 검증 | 🟡 중간 |
| `totp()` | 2FA TOTP 생성 | 🟡 중간 |

### 5.2 인코딩 관련

| 함수 | 설명 | 우선순위 |
|------|------|----------|
| `base58_encode()` | Base58 인코딩 | 🟢 낮음 |
| `base58_decode()` | Base58 디코딩 | 🟢 낮음 |
| `binaryToBase16()` | 바이너리→Hex | 🟢 낮음 |
| `base16ToBinary()` | Hex→바이너리 | 🟢 낮음 |

### 5.3 숫자 처리 관련

| 함수 | 설명 | 우선순위 |
|------|------|----------|
| `decimal_to_precision()` | 정밀도 변환 | 🔴 높음 |
| `number_to_string()` | 숫자→문자열 | 🟡 중간 |
| `parse_number()` | 문자열→숫자 | 🟡 중간 |
| `omit_zero()` | 0 제거 | 🟢 낮음 |

### 5.4 일반 유틸리티

| 함수 | 설명 | 우선순위 |
|------|------|----------|
| `deep_extend()` | 깊은 객체 병합 | 🟡 중간 |
| `extend()` | 객체 병합 | 🟡 중간 |
| `omit()` | 키 제외 | 🟢 낮음 |
| `group_by()` | 그룹핑 | 🟢 낮음 |
| `index_by()` | 인덱싱 | 🟢 낮음 |
| `sort_by()` | 정렬 | 🟢 낮음 |
| `filter_by()` | 필터링 | 🟢 낮음 |
| `array_concat()` | 배열 연결 | 🟢 낮음 |
| `in_array()` | 포함 여부 | 🟢 낮음 |

---

## 6. 인프라스트럭처 Gap

### 6.1 프록시 지원

```rust
// 미구현
struct ProxyConfig {
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    socks_proxy: Option<String>,
    no_proxy: Vec<String>,
}

impl ExchangeConfig {
    fn with_proxy(self, config: ProxyConfig) -> Self;
    fn with_http_proxy(self, url: &str) -> Self;
    fn with_socks_proxy(self, url: &str) -> Self;
}
```

### 6.2 Sandbox/Testnet 지원

| 거래소 | Sandbox URL 필요 |
|--------|------------------|
| Binance | testnet.binance.vision |
| Bybit | testnet.bybit.com |
| OKX | aws.okx.com |
| Gate | fx-api-testnet.gateio.ws |
| KuCoin | sandbox.kucoin.com |

### 6.3 캐싱 시스템

```rust
// 미구현
struct MarketCache {
    markets: HashMap<String, Market>,
    last_update: Instant,
    ttl: Duration,
}

trait Cacheable {
    fn cache_key(&self) -> String;
    fn is_expired(&self) -> bool;
    fn refresh(&mut self) -> CcxtResult<()>;
}
```

### 6.4 요청 재시도 로직

```rust
// 부분 구현 - 고도화 필요
struct RetryConfig {
    max_retries: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
    retry_on: Vec<CcxtErrorCode>,
    exponential_backoff: bool,
}
```

---

## 7. WebSocket 기능 Gap

### 7.1 미구현 WebSocket 메서드

| 메서드 | 설명 | 우선순위 |
|--------|------|----------|
| `watch_order_book_for_symbols()` | 다중 심볼 호가 | 🟡 중간 |
| `watch_liquidations()` | 청산 이벤트 | 🟡 중간 |
| `watch_liquidations_for_symbols()` | 다중 심볼 청산 | 🟢 낮음 |
| `watch_mark_prices()` | 마크 가격 목록 | 🟢 낮음 |

### 7.2 Order Book 동기화

```rust
// 미구현 - 고급 기능
struct OrderBookManager {
    /// Checksum 검증
    fn verify_checksum(&self, orderbook: &OrderBook, checksum: &str) -> bool;

    /// Delta 적용
    fn apply_delta(&mut self, delta: OrderBookDelta) -> CcxtResult<()>;

    /// 스냅샷 초기화
    fn reset_from_snapshot(&mut self, snapshot: OrderBook);

    /// 갭 감지
    fn detect_gap(&self, sequence: u64) -> bool;
}
```

### 7.3 연결 복구

```rust
// 부분 구현 - 고도화 필요
struct WsReconnectConfig {
    auto_reconnect: bool,
    max_reconnect_attempts: u32,
    reconnect_interval_ms: u64,
    ping_interval_ms: u64,
    subscription_recovery: bool,
}
```

---

## 8. 우선순위별 작업 목록

### 8.1 ✅ 완료된 Phase (10-14)

#### Phase 10-11: 선물 기능 확장 ✅ 완료
- [x] Bitget, Kucoin, MEXC 선물 기능 추가
- [x] `fetch_positions`, `fetch_funding_rate`, `fetch_open_interest` 등

#### Phase 12: 고급 주문 기능 ✅ 완료
- [x] `edit_order()` 구현 (Binance, OKX)
- [x] `create_orders()` 구현 (Binance, OKX)
- [x] `cancel_all_orders()` 구현 (Binance, OKX)

#### Phase 13: 고급 시장 데이터 ✅ 완료
- [x] `fetch_mark_price()` / `fetch_mark_prices()` (OKX, Bybit, Bitget, Kucoin, MEXC)
- [x] `fetch_mark_ohlcv()` (OKX, Bybit, Bitget)
- [x] `fetch_index_ohlcv()` (OKX, Bybit, Bitget)

#### Phase 14: 추가 계정 기능 ✅ 완료
- [x] `transfer()` (Binance, OKX, Bybit 등)
- [x] `add_margin()` / `reduce_margin()` (Binance, OKX, Bybit 등)
- [x] `set_position_mode()` (Binance, OKX, Bybit 등)
- [x] `withdraw()` (HTX, MEXC 등)
- [x] `fetch_deposit_address()` (HTX, MEXC, Kucoin, Gate)

#### Phase 15: 마진 대출 기능 ✅ 완료
- [x] `borrow_cross_margin()` (Binance, OKX, Bybit)
- [x] `borrow_isolated_margin()` (Binance)
- [x] `repay_cross_margin()` (Binance, OKX, Bybit)
- [x] `repay_isolated_margin()` (Binance)
- [x] `fetch_cross_borrow_rate()` (Binance, OKX, Bybit)
- [x] `fetch_isolated_borrow_rate()` (Binance)

### 8.2 🔴 높음 (Phase 16-17)

#### Phase 16: 주요 거래소 추가
1. Hyperliquid (DEX)
2. BitMEX
3. Deribit (옵션 선물)
4. Crypto.com
5. Gemini

### 8.3 🟡 중간 (Phase 17-18)

#### Phase 17: 옵션 거래 기능
1. Options 타입 및 메서드 추가
2. Greeks 계산
3. `fetch_option()`, `fetch_option_chain()` 구현
4. Sandbox/Testnet 지원

#### Phase 18: 인프라 고도화
1. 프록시 지원
2. 캐싱 시스템
3. 요청 재시도 로직 개선
4. Order Book 동기화 (checksum)
5. WebSocket 연결 복구 개선

### 8.4 🟢 낮음 (Phase 19+)

#### Phase 19: Conversion 기능
1. `fetch_convert_currencies()` 구현
2. `fetch_convert_quote()`, `create_convert_trade()` 구현
3. 변환 거래 이력 조회

#### Phase 20: 나머지 거래소
- 거래량 순으로 나머지 거래소 추가

#### Phase 21: 최적화 및 문서화
- 성능 최적화
- API 문서화
- 예제 코드 작성
- 벤치마크 테스트

---

## 부록: 구현 완료 체크리스트

### Exchange Trait 메서드 (70+개 구현)

#### ✅ 기본 메서드 (구현 완료)
- [x] id(), name(), version()
- [x] countries(), rate_limit(), has()
- [x] urls(), timeframes()
- [x] load_markets(), fetch_markets()
- [x] fetch_currencies()
- [x] fetch_ticker(), fetch_tickers()
- [x] fetch_order_book()
- [x] fetch_trades()
- [x] fetch_ohlcv()
- [x] fetch_balance()
- [x] create_order(), create_limit_order(), create_market_order()
- [x] cancel_order(), cancel_orders()
- [x] fetch_order(), fetch_orders()
- [x] fetch_open_orders(), fetch_closed_orders(), fetch_canceled_orders()
- [x] fetch_my_trades()
- [x] fetch_deposits(), fetch_withdrawals()
- [x] withdraw()
- [x] fetch_deposit_address()
- [x] fetch_funding_rate(), fetch_funding_rates()
- [x] fetch_funding_rate_history()
- [x] fetch_open_interest(), fetch_open_interest_history()
- [x] fetch_liquidations(), fetch_my_liquidations()
- [x] fetch_positions(), fetch_position()
- [x] set_leverage(), fetch_leverage()
- [x] set_margin_mode(), fetch_margin_mode()
- [x] fetch_index_price()

#### ✅ Phase 12: 고급 주문 (구현 완료)
- [x] edit_order() - Binance, OKX
- [x] create_orders() - Binance, OKX
- [x] cancel_all_orders() - Binance, OKX

#### ✅ Phase 13: 고급 시장 데이터 (구현 완료)
- [x] fetch_mark_price() - OKX, Bybit, Bitget, Kucoin, MEXC
- [x] fetch_mark_prices() - OKX, Bybit, Bitget, Kucoin, MEXC
- [x] fetch_mark_ohlcv() - OKX, Bybit, Bitget (Kucoin, MEXC는 NotSupported)
- [x] fetch_index_ohlcv() - OKX, Bybit, Bitget (Kucoin, MEXC는 NotSupported)

#### ✅ Phase 14: 계정/마진 (구현 완료)
- [x] transfer() - Binance, OKX, Bybit 등
- [x] add_margin() - Binance, OKX, Bybit 등
- [x] reduce_margin() - Binance, OKX, Bybit 등
- [x] set_position_mode() - Binance, OKX, Bybit 등

#### ✅ Phase 15: 마진 대출/상환 (구현 완료)
- [x] borrow_cross_margin() - Binance, OKX, Bybit
- [x] borrow_isolated_margin() - Binance
- [x] repay_cross_margin() - Binance, OKX, Bybit
- [x] repay_isolated_margin() - Binance
- [x] fetch_cross_borrow_rate() - Binance, OKX, Bybit
- [x] fetch_isolated_borrow_rate() - Binance

#### ❌ 미구현
- [ ] fetch_transfers()
- [ ] fetch_ledger()
- [ ] fetch_cross_borrow_rates(), fetch_isolated_borrow_rates()
- [ ] set_margin()
- [ ] fetch_greeks(), fetch_option(), fetch_option_chain()
- [ ] convert 관련 메서드

---

*이 문서는 ccxt-rust 프로젝트의 현재 상태와 ccxt-reference 대비 부족한 점을 분석한 것입니다.*
*최종 업데이트: 2025년 12월 28일 (Phase 15 완료)*
*정기적으로 업데이트하여 프로젝트 진행 상황을 추적하세요.*
