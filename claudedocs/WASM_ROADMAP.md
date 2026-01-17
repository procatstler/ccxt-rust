# ccxt-rust WASM Roadmap

## 현재 상태 (2025-01-17)

### 완료된 작업
- [x] WASM 빌드 환경 구성 (`native`/`wasm` feature flags)
- [x] WASM 호환 HTTP 클라이언트 (`gloo-net` 기반)
- [x] WASM 호환 WebSocket 클라이언트 (`web-sys` 기반)
- [x] 기본 JS 바인딩 (`wasm-bindgen`)
- [x] wasm-pack 빌드 성공
- [x] **Phase 1 완료**: 거래소 REST API 바인딩 (Binance, Upbit, Bybit, OKX, Kraken)
- [x] **Phase 1.2 완료**: 추가 REST API (fetchTickers, fetchTrades, fetchOhlcv)
- [x] **Phase 2 완료**: WebSocket 스트리밍 지원

### 빌드 정보
```bash
# 빌드 명령
wasm-pack build --target web --no-default-features --features "wasm,cex"

# 출력
pkg/
├── ccxt_rust_bg.wasm    # 699KB (optimized)
├── ccxt_rust.js         # 74KB  (JS glue)
├── ccxt_rust.d.ts       # 21KB  (TypeScript types)
└── package.json
```

### 지원 거래소 및 메서드

| 거래소 | REST 클래스 | WebSocket 클래스 | REST API | WebSocket API |
|--------|-------------|------------------|----------|---------------|
| Binance | `WasmBinance` | `WasmBinanceWs` | fetchTicker, fetchOrderBook, fetchMarkets, fetchTickers, fetchTrades, fetchOhlcv | watchTicker, watchOrderBook, watchTrades |
| Upbit | `WasmUpbit` | `WasmUpbitWs` | fetchTicker, fetchOrderBook, fetchMarkets, fetchTickers, fetchTrades, fetchOhlcv | watchTicker, watchOrderBook |
| Bybit | `WasmBybit` | `WasmBybitWs` | fetchTicker, fetchOrderBook, fetchMarkets, fetchTickers, fetchTrades, fetchOhlcv | watchTicker, watchOrderBook |
| OKX | `WasmOkx` | - | fetchTicker, fetchOrderBook, fetchMarkets, fetchTickers, fetchTrades, fetchOhlcv | - |
| Kraken | `WasmKraken` | - | fetchTicker, fetchOrderBook, fetchMarkets, fetchTickers, fetchTrades, fetchOhlcv | - |

### 현재 익스포트된 API
```typescript
// 설정 클래스
class WasmExchangeConfig {
  setApiKey(key: string): void
  setApiSecret(secret: string): void
  setPassword(password: string): void
  setUid(uid: string): void
  setSandbox(sandbox: boolean): void
  setTimeout(timeout_ms: bigint): void
}

// 데이터 타입
class WasmTicker { symbol, last, bid, ask, high, low, volume, timestamp, toJson() }
class WasmOrderBook { symbol, timestamp, getBids(), getAsks(), toJson() }
class WasmMarket { id, symbol, base, quote, active }

// 유틸리티 함수
function hmacSha256(secret: string, message: string): string
function hmacSha512(secret: string, message: string): string
function sha256(data: string): string
function sha512(data: string): string
function base64Encode(data: string): string
function base64Decode(data: string): string
function urlEncode(data: string): string
function generateUuid(): string
function getCurrentTimestamp(): bigint
function getSupportedExchanges(): string
function version(): string
```

---

## Phase 1: WASM 거래소 기능 추가 ✅ 완료

### 1.1 거래소 클래스 바인딩 ✅
**상태**: 완료 (2025-01-17)

```typescript
// 구현된 거래소 클래스 (공통 인터페이스)
interface WasmExchange {
    constructor(config?: WasmExchangeConfig);
    fetchTicker(symbol: string): Promise<WasmTicker>;
    fetchOrderBook(symbol: string, limit?: number): Promise<WasmOrderBook>;
    fetchMarkets(): Promise<any>;
    fetchTickers(symbols?: string[]): Promise<any>;
    fetchTrades(symbol: string, limit?: number): Promise<any>;
    fetchOhlcv(symbol: string, timeframe: string, since?: number, limit?: number): Promise<any>;
}

// 지원 거래소: WasmBinance, WasmUpbit, WasmBybit, WasmOkx, WasmKraken
```

**완료된 거래소**:
1. ✅ `WasmBinance` - 글로벌 1위
2. ✅ `WasmUpbit` - 한국 1위
3. ✅ `WasmBybit` - 선물 거래
4. ✅ `WasmOkx` - 글로벌 3위
5. ✅ `WasmKraken` - 미국/유럽

### 1.2 Public API 구현
**목표**: 인증 없이 사용 가능한 시장 데이터 API

| 메서드 | 설명 | 반환 타입 |
|--------|------|-----------|
| `fetch_markets()` | 거래 가능한 마켓 목록 | `WasmMarket[]` |
| `fetch_ticker(symbol)` | 단일 심볼 시세 | `WasmTicker` |
| `fetch_tickers(symbols?)` | 다중 심볼 시세 | `WasmTicker[]` |
| `fetch_orderbook(symbol, limit?)` | 호가창 | `WasmOrderBook` |
| `fetch_trades(symbol, limit?)` | 최근 체결 내역 | `WasmTrade[]` |
| `fetch_ohlcv(symbol, timeframe, since?, limit?)` | 캔들 데이터 | `WasmOHLCV[]` |

### 1.3 Private API 구현 (선택적)
**목표**: 인증된 거래 API (주의: 브라우저 환경 보안 고려)

| 메서드 | 설명 | 비고 |
|--------|------|------|
| `fetch_balance()` | 잔고 조회 | API Key 필요 |
| `create_order(...)` | 주문 생성 | 보안 주의 |
| `cancel_order(id)` | 주문 취소 | |
| `fetch_orders(symbol?)` | 주문 조회 | |

**보안 고려사항**:
- 브라우저에서 API Secret 노출 위험
- 서버 프록시 사용 권장
- 읽기 전용 API Key 권장

---

## Phase 2: WebSocket 지원 ✅ 완료

### 2.1 실시간 데이터 스트리밍 ✅
**상태**: 완료 (2025-01-17)

```typescript
// 구현된 WebSocket API
import { WasmBinanceWs, WasmBybitWs, WasmUpbitWs } from './pkg/ccxt_rust.js';

// Binance WebSocket
const binanceWs = new WasmBinanceWs();
binanceWs.watchTicker('BTC/USDT', (msg) => {
    if (msg.message_type === 'ticker') {
        const data = JSON.parse(msg.data);
        console.log('BTC/USDT:', data.c);  // Last price
    }
});

// Bybit WebSocket
const bybitWs = new WasmBybitWs();
bybitWs.watchOrderBook('BTC/USDT', 50, (msg) => {
    if (msg.message_type === 'orderbook') {
        const data = JSON.parse(msg.data);
        console.log('OrderBook:', data);
    }
});
```

### 2.2 WebSocket 메시지 타입
```typescript
interface WasmWsMessage {
    message_type: 'connected' | 'disconnected' | 'ticker' | 'orderbook' | 'trade' | 'error';
    symbol: string;
    data: string;  // JSON string
    timestamp?: number;
}
```

### 2.3 지원 메서드
| 거래소 | watchTicker | watchOrderBook | watchTrades |
|--------|:-----------:|:--------------:|:-----------:|
| Binance | ✅ | ✅ | ✅ |
| Upbit | ✅ | ✅ | ❌ |
| Bybit | ✅ | ✅ | ❌ |

---

## Phase 3: 테스트 & 문서화 (P1)

### 3.1 브라우저 테스트 페이지
**파일**: `examples/wasm_demo.html`

```html
<!DOCTYPE html>
<html>
<head>
  <title>ccxt-rust WASM Demo</title>
</head>
<body>
  <h1>ccxt-rust WASM Demo</h1>
  <div id="ticker"></div>

  <script type="module">
    import init, {
      WasmBinance,
      WasmExchangeConfig,
      getSupportedExchanges
    } from './pkg/ccxt_rust.js';

    async function main() {
      await init();

      console.log('Supported:', JSON.parse(getSupportedExchanges()));

      const config = new WasmExchangeConfig();
      const binance = new WasmBinance(config);

      const ticker = await binance.fetch_ticker("BTC/USDT");
      document.getElementById('ticker').innerText =
        `BTC/USDT: $${ticker.last}`;
    }

    main();
  </script>
</body>
</html>
```

### 3.2 문서 업데이트
**README.md 추가 섹션**:
- WASM 빌드 방법
- 브라우저 사용법
- Node.js 사용법
- API 레퍼런스

---

## Phase 4: 배포 (P2)

### 4.1 npm 패키지 설정
```json
{
  "name": "@anthropic/ccxt-rust",
  "version": "0.1.0",
  "description": "CCXT cryptocurrency exchange library in Rust/WASM",
  "main": "ccxt_rust.js",
  "types": "ccxt_rust.d.ts",
  "files": [
    "ccxt_rust_bg.wasm",
    "ccxt_rust.js",
    "ccxt_rust.d.ts"
  ],
  "keywords": ["cryptocurrency", "exchange", "trading", "wasm", "rust"],
  "repository": "https://github.com/anthropics/ccxt-rust"
}
```

### 4.2 CI/CD 파이프라인
```yaml
# .github/workflows/wasm.yml
name: WASM Build & Publish

on:
  release:
    types: [published]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rustwasm/wasm-pack-action@v0.4.0
      - run: wasm-pack build --target web --no-default-features --features "wasm,cex"
      - run: npm publish ./pkg
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

### 4.3 CDN 배포
- unpkg: `https://unpkg.com/@anthropic/ccxt-rust@latest`
- jsdelivr: `https://cdn.jsdelivr.net/npm/@anthropic/ccxt-rust@latest`

---

## 기술적 제약사항

### WASM에서 사용 불가
| 기능 | 이유 | 대안 |
|------|------|------|
| `tokio` 런타임 | 네이티브 전용 | `wasm-bindgen-futures` |
| `ring` 크레이트 | C 의존성 | `sha2`, `hmac` 직접 사용 |
| `jsonwebtoken` | `ring` 의존 | JWT 서버 처리 |
| 파일 시스템 | 브라우저 제한 | LocalStorage/IndexedDB |
| 환경 변수 | 브라우저 제한 | JS에서 설정 주입 |

### CORS 제한
- 대부분의 거래소 API는 CORS 제한 있음
- 해결책:
  1. 프록시 서버 사용
  2. 브라우저 확장 프로그램
  3. 거래소의 공식 CORS 허용 엔드포인트 사용

---

## 일정 추정

| Phase | 작업 | 예상 기간 |
|-------|------|-----------|
| 1.1 | 거래소 바인딩 (5개) | 2-3일 |
| 1.2 | Public API | 1-2일 |
| 2.1 | WebSocket 지원 | 2-3일 |
| 3.1 | 테스트 페이지 | 0.5일 |
| 3.2 | 문서화 | 0.5일 |
| 4.x | 배포 설정 | 1일 |
| **총계** | | **7-10일** |

---

## 참고 자료

- [wasm-bindgen 가이드](https://rustwasm.github.io/docs/wasm-bindgen/)
- [wasm-pack 문서](https://rustwasm.github.io/docs/wasm-pack/)
- [gloo-net API](https://docs.rs/gloo-net/latest/gloo_net/)
- [web-sys WebSocket](https://rustwasm.github.io/wasm-bindgen/api/web_sys/struct.WebSocket.html)
