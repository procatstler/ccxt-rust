# WASM 아키텍처 개선 계획

## 목표
- 네이티브와 WASM 간 코드 중복 최소화
- 새 거래소 추가 시 작업량 감소 (현재 ~300줄 → 목표 ~50줄)
- 유지보수성 향상

## 현재 상태 분석

### 문제점
```
src/wasm.rs (2,501줄)
├── WasmExchangeConfig      # 공통 설정 (재사용 가능)
├── WasmTicker/OrderBook    # 공통 타입 (재사용 가능)
├── 유틸리티 함수           # 공통 (재사용 가능)
├── WasmBinance             # 거래소별 ~300줄 중복
├── WasmUpbit               # 거래소별 ~300줄 중복
├── WasmBybit               # 거래소별 ~300줄 중복
├── WasmOkx                 # 거래소별 ~300줄 중복
└── WasmKraken              # 거래소별 ~300줄 중복
```

### 중복 패턴
1. **심볼 변환**: BTC/USDT → BTCUSDT (Binance), KRW-BTC (Upbit), BTC-USDT (OKX)
2. **HTTP 요청**: 모든 거래소가 동일한 패턴
3. **응답 파싱**: Ticker, OrderBook, Trades, OHLCV 구조 유사
4. **wasm_bindgen 보일러플레이트**: 거의 동일한 메서드 시그니처

---

## 개선 계획

### Phase 1: 공통 모듈 추출 ✅ 진행중
**파일**: `src/wasm/common.rs`

```rust
// 1. 거래소 설정 트레이트
pub trait WasmExchangeExt {
    fn base_url(&self) -> &str;
    fn convert_symbol(&self, symbol: &str) -> String;
    fn parse_ticker(&self, data: serde_json::Value) -> WasmTicker;
    fn parse_orderbook(&self, data: serde_json::Value) -> WasmOrderBook;
}

// 2. 공통 HTTP 래퍼
pub async fn fetch_json<T>(url: &str) -> Result<T, JsValue>;

// 3. 공통 응답 타입
pub struct WasmTicker { ... }
pub struct WasmOrderBook { ... }
pub struct WasmTrade { ... }
pub struct WasmOhlcv { ... }
```

### Phase 2: 거래소 정의 매크로
**파일**: `src/wasm/macros.rs`

```rust
// 매크로로 거래소 구현 생성
define_wasm_exchange! {
    name: Binance,
    base_url: "https://api.binance.com",
    symbol_convert: |s| s.replace("/", ""),
    endpoints: {
        ticker: "/api/v3/ticker/24hr?symbol={symbol}",
        orderbook: "/api/v3/depth?symbol={symbol}&limit={limit}",
        markets: "/api/v3/exchangeInfo",
        trades: "/api/v3/trades?symbol={symbol}&limit={limit}",
        ohlcv: "/api/v3/klines?symbol={symbol}&interval={interval}&limit={limit}",
    },
    ticker_parser: BinanceTickerParser,
    orderbook_parser: BinanceOrderBookParser,
}
```

### Phase 3: 파서 모듈화
**파일**: `src/wasm/parsers/`

```
src/wasm/parsers/
├── mod.rs
├── binance.rs    # Binance 응답 파서
├── upbit.rs      # Upbit 응답 파서
├── bybit.rs      # Bybit 응답 파서
├── okx.rs        # OKX 응답 파서
├── kraken.rs     # Kraken 응답 파서
└── common.rs     # 공통 파싱 유틸리티
```

### Phase 4: 디렉토리 구조 변경
**현재**:
```
src/
└── wasm.rs (2,501줄 단일 파일)
```

**개선 후**:
```
src/wasm/
├── mod.rs           # 모듈 진입점 + re-exports
├── config.rs        # WasmExchangeConfig
├── types.rs         # WasmTicker, WasmOrderBook, etc.
├── utils.rs         # hmac, sha, base64, etc.
├── http.rs          # 공통 HTTP 래퍼
├── macros.rs        # 거래소 생성 매크로
├── exchanges/
│   ├── mod.rs
│   ├── binance.rs   # ~50줄
│   ├── upbit.rs     # ~50줄
│   ├── bybit.rs     # ~50줄
│   ├── okx.rs       # ~50줄
│   ├── kraken.rs    # ~50줄
│   └── ...          # 새 거래소 추가 용이
└── websocket/
    ├── mod.rs
    ├── binance.rs
    ├── upbit.rs
    └── bybit.rs
```

---

## 작업 체크리스트

### Step 1: 디렉토리 구조 생성 ✅
- [x] `src/wasm/` 디렉토리 생성
- [x] `src/wasm/mod.rs` 생성
- [x] 기존 `src/wasm.rs` 내용 분리

### Step 2: 공통 타입 추출 ✅
- [x] `src/wasm/types.rs` - WasmTicker, WasmOrderBook, WasmMarket
- [x] `src/wasm/config.rs` - WasmExchangeConfig

### Step 3: 유틸리티 분리 ✅
- [x] `src/wasm/utils.rs` - hmac, sha, base64, uuid 등

### Step 4: 거래소 모듈 생성 ✅
- [x] `src/wasm/exchanges/mod.rs` - 거래소 모듈 구조
- [x] 공통 파싱 유틸리티

### Step 5: Binance 마이그레이션 ✅
- [x] `src/wasm/exchanges/binance.rs` 생성
- [x] 빌드 검증

### Step 6: 나머지 거래소 마이그레이션 ✅
- [x] Upbit 마이그레이션
- [x] Bybit 마이그레이션
- [x] OKX 마이그레이션
- [x] Kraken 마이그레이션

### Step 7: WebSocket 분리 ✅
- [x] `src/wasm/websocket/` 디렉토리
- [x] WasmBinanceWs 마이그레이션
- [x] WasmUpbitWs 마이그레이션
- [x] WasmBybitWs 마이그레이션

### Step 8: 빌드 검증 ✅
- [x] WASM 빌드 성공 확인
- [x] Native 빌드 성공 확인
- [x] 기존 wasm.rs 파일 제거

### Step 9: 새 거래소 추가 테스트 (향후 작업)
- [ ] Coinbase 추가
- [ ] Gate.io 추가

---

## 결과

| 항목 | 이전 | 개선 후 |
|------|------|---------|
| 파일 구조 | 단일 파일 (2,501줄) | 모듈화된 디렉토리 |
| 거래소 파일 수 | 1 | 5 (각 250-300줄) |
| 공통 모듈 | 없음 | config, types, utils |
| WebSocket | 단일 파일 내 | websocket/ 디렉토리 |
| 유지보수성 | 낮음 | 높음 |

### 생성된 파일 구조
```
src/wasm/
├── mod.rs           # 모듈 진입점 + re-exports
├── config.rs        # WasmExchangeConfig
├── types.rs         # WasmTicker, WasmOrderBook, WasmMarket
├── utils.rs         # hmac, sha, base64, uuid 등
├── exchanges/
│   ├── mod.rs       # 거래소 모듈 + 공통 파싱
│   ├── binance.rs   # ~250줄
│   ├── upbit.rs     # ~290줄
│   ├── bybit.rs     # ~270줄
│   ├── okx.rs       # ~260줄
│   └── kraken.rs    # ~260줄
└── websocket/
    └── mod.rs       # WebSocket 클라이언트 (~450줄)
```

---

## 진행 상태

- **현재 단계**: ✅ 완료
- **마지막 업데이트**: 2025-01-17
- **담당**: Claude

### 진행 로그
| 시간 | 작업 | 상태 |
|------|------|------|
| Session 1 | Step 1: 디렉토리 구조 생성 | ✅ 완료 |
| Session 1 | Step 2: 공통 타입 추출 | ✅ 완료 |
| Session 1 | Step 3: 유틸리티 분리 | ✅ 완료 |
| Session 1 | Step 4: 거래소 모듈 생성 | ✅ 완료 |
| Session 1 | Step 5: Binance 마이그레이션 | ✅ 완료 |
| Session 1 | Step 6: 나머지 거래소 마이그레이션 | ✅ 완료 |
| Session 1 | Step 7: WebSocket 분리 | ✅ 완료 |
| Session 1 | Step 8: 빌드 검증 | ✅ 완료 |
