# CCXT-Rust 완벽 포팅 로드맵

## 1. 전체 규모 분석

### 1.1 CCXT Reference 통계
| 항목 | 수량 |
|------|------|
| 총 TypeScript 코드 | **476,244 줄** |
| 거래소 수 | **103개** |
| WebSocket 파일 (pro/) | **76개** |
| Base Exchange 클래스 | **8,675 줄** |
| 타입 정의 | **651 줄** |
| 에러 정의 | **252 줄** |
| Precise 클래스 | **302 줄** |

### 1.2 거래소 코드 크기 (상위 15개)
| 순위 | 거래소 | 코드 라인 | WebSocket |
|------|--------|----------|-----------|
| 1 | Binance | 14,411 | Yes |
| 2 | Bitget | 11,063 | Yes |
| 3 | Bybit | 9,577 | Yes |
| 4 | HTX | 9,502 | Yes |
| 5 | OKX | 9,178 | Yes |
| 6 | Gate | 8,325 | Yes |
| 7 | BingX | 6,755 | Yes |
| 8 | MEXC | 6,198 | Yes |
| 9 | CoinEx | 6,188 | Yes |
| 10 | KuCoin | 5,811 | Yes |
| 11 | BitMart | 5,653 | Yes |
| 12 | CoinCatch | 5,495 | Yes |
| 13 | Phemex | 5,331 | Yes |
| 14 | Coinbase | 5,270 | Yes |
| 15 | XT | 5,153 | Yes |

### 1.3 한국 거래소 (현재 구현됨)
| 거래소 | 코드 라인 | 상태 |
|--------|----------|------|
| Upbit | 2,384 | ✅ 완료 |
| Bithumb | 1,285 | ✅ 완료 |
| Coinone | 1,292 | ✅ 완료 |

---

## 2. Exchange Base 클래스 메서드 분류

### 2.1 Public Market Data (공개 API - 29개)
```
fetchMarkets          fetchCurrencies       fetchTicker
fetchTickers          fetchOrderBook        fetchOrderBooks
fetchTrades           fetchOHLCV            fetchStatus
fetchTime             fetchBidsAsks         fetchLastPrices
fetchMarkPrice        fetchMarkPrices       fetchIndexOHLCV
fetchMarkOHLCV        fetchPremiumIndexOHLCV
fetchTradingFees      fetchTradingFee       fetchFundingRate
fetchFundingRates     fetchFundingRateHistory
fetchOpenInterest     fetchOpenInterests    fetchOpenInterestHistory
fetchLiquidations     fetchLongShortRatio   fetchLongShortRatioHistory
fetchOption           fetchOptionChain
```

### 2.2 Private Trading (비공개 거래 API - 45개)
```
// 주문 생성
createOrder           createOrders          createLimitOrder
createLimitBuyOrder   createLimitSellOrder  createMarketOrder
createMarketBuyOrder  createMarketSellOrder createPostOnlyOrder
createReduceOnlyOrder createStopOrder       createStopLimitOrder
createStopMarketOrder createStopLossOrder   createTakeProfitOrder
createTriggerOrder    createTrailingAmountOrder
createTrailingPercentOrder
createOrderWithTakeProfitAndStopLoss
createMarketBuyOrderWithCost
createMarketSellOrderWithCost

// 주문 수정
editOrder             editOrders            editLimitOrder
editLimitBuyOrder     editLimitSellOrder

// 주문 취소
cancelOrder           cancelOrders          cancelAllOrders
cancelOrdersForSymbols
cancelAllOrdersAfter
```

### 2.3 Private Account (비공개 계정 API - 25개)
```
fetchBalance          fetchAccounts         fetchLedger
fetchDeposits         fetchWithdrawals      fetchDepositsWithdrawals
fetchDepositAddress   fetchDepositAddresses
fetchTransfer         fetchTransfers
withdraw              transfer
fetchMyTrades         fetchOrderTrades
fetchOrder            fetchOrders           fetchOpenOrders
fetchClosedOrders     fetchCanceledAndClosedOrders
```

### 2.4 Margin & Leverage (마진 거래 - 20개)
```
borrowMargin          borrowCrossMargin     borrowIsolatedMargin
repayMargin           repayCrossMargin      repayIsolatedMargin
fetchBorrowRate       fetchBorrowRates
fetchCrossBorrowRate  fetchCrossBorrowRates
fetchIsolatedBorrowRate fetchIsolatedBorrowRates
fetchBorrowInterest
setLeverage           fetchLeverage         fetchLeverages
fetchLeverageTiers    fetchMarketLeverageTiers
setMarginMode         fetchMarginMode       fetchMarginModes
addMargin             reduceMargin          setMargin
fetchMarginAdjustmentHistory
```

### 2.5 Positions (포지션 - 15개)
```
fetchPosition         fetchPositions        fetchPositionsForSymbol
fetchPositionsRisk    fetchPositionHistory  fetchPositionsHistory
closePosition         closeAllPositions
setPositionMode       fetchPositionMode
fetchMyLiquidations
```

### 2.6 WebSocket (실시간 스트리밍 - 40개+)
```
// Watch 메서드
watchTicker           watchTickers          watchOrderBook
watchTrades           watchOHLCV            watchBalance
watchOrders           watchMyTrades         watchPositions
watchBidsAsks         watchMarkPrice        watchMarkPrices
watchFundingRate      watchFundingRates     watchLiquidations

// UnWatch 메서드
unWatchTicker         unWatchTickers        unWatchOrderBook
unWatchTrades         unWatchOHLCV          unWatchOrders
unWatchMyTrades       unWatchPositions      ...

// WebSocket 버전 REST API
createOrderWs         cancelOrderWs         fetchBalanceWs
fetchTickerWs         fetchOrderBookWs      ...
```

### 2.7 Helper Methods (헬퍼 - 50개+)
```
// Safe 메서드
safeString            safeNumber            safeInteger
safeValue             safeTimestamp         safeTicker
safeOrder             safeTrade             safeBalance
safeMarket            safeCurrency          ...

// Parse 메서드
parseTicker           parseOrder            parseTrade
parseBalance          parseOHLCV            parseOrderBook
parsePosition         parseLedgerEntry      ...

// 기타
loadMarkets           market                currency
amountToPrecision     priceToPrecision      costToPrecision
```

---

## 3. 타입 시스템 (50+ 타입)

### 3.1 핵심 데이터 타입
| 타입 | 설명 | 현재 상태 |
|------|------|----------|
| Market | 마켓 정보 | ✅ 구현됨 |
| Ticker | 시세 정보 | ✅ 구현됨 |
| OrderBook | 호가창 | ✅ 구현됨 |
| Trade | 체결 내역 | ✅ 구현됨 |
| Order | 주문 정보 | ✅ 구현됨 |
| OHLCV | 캔들 데이터 | ✅ 구현됨 |
| Balance | 잔고 | ✅ 구현됨 |
| Currency | 화폐 정보 | ✅ 구현됨 |
| Fee | 수수료 | ✅ 구현됨 |
| Transaction | 입출금 | ✅ 구현됨 |

### 3.2 추가 필요 타입
| 타입 | 설명 | 우선순위 |
|------|------|----------|
| Position | 선물 포지션 | 높음 |
| Leverage | 레버리지 설정 | 높음 |
| LeverageTier | 레버리지 티어 | 중간 |
| FundingRate | 펀딩비 | 중간 |
| FundingHistory | 펀딩 내역 | 중간 |
| BorrowInterest | 대출 이자 | 중간 |
| OpenInterest | 미결제약정 | 중간 |
| Liquidation | 청산 이벤트 | 중간 |
| Greeks | 옵션 그릭스 | 낮음 |
| Option | 옵션 계약 | 낮음 |
| LongShortRatio | 롱숏비율 | 낮음 |
| Conversion | 환전 | 낮음 |

---

## 4. 에러 계층 구조 (42개)

### 현재 구현된 에러
- ✅ ExchangeError, AuthenticationError, PermissionDenied
- ✅ InsufficientFunds, InvalidOrder, OrderNotFound
- ✅ BadSymbol, BadRequest, NotSupported
- ✅ NetworkError, RateLimitExceeded, RequestTimeout
- ✅ DDoSProtection, ExchangeNotAvailable

### 추가 필요 에러
- ⬜ AccountNotEnabled, AccountSuspended
- ⬜ OrderNotCached, OrderImmediatelyFillable, OrderNotFillable
- ⬜ DuplicateOrderId, ContractUnavailable
- ⬜ NoChange, MarginModeAlreadySet, MarketClosed
- ⬜ InvalidNonce, ChecksumError
- ⬜ BadResponse, NullResponse, CancelPending

---

## 5. 구현 로드맵

### Phase 1: 핵심 인프라 강화 (1-2주)
**목표: Base Exchange 완성도 90%**

#### 1.1 Precise 클래스 구현
- [ ] BigInt 기반 고정밀 연산
- [ ] stringMul, stringDiv, stringAdd, stringSub
- [ ] 비교 연산 (gt, ge, lt, le, eq)

#### 1.2 타입 시스템 확장
- [ ] Position 타입
- [ ] Leverage, LeverageTier 타입
- [ ] FundingRate, FundingHistory 타입
- [ ] OpenInterest, Liquidation 타입

#### 1.3 에러 시스템 완성
- [ ] 누락된 28개 에러 타입 추가
- [ ] 에러 계층 구조 완성

#### 1.4 Exchange Base 트레이트 확장
- [ ] 마진/레버리지 메서드 추가
- [ ] 포지션 관련 메서드 추가
- [ ] 펀딩비 메서드 추가

#### 1.5 한국 거래소 비공개 API 완성
- [ ] Upbit 비공개 API (잔고, 주문)
- [ ] Bithumb 비공개 API
- [ ] Coinone 비공개 API

### Phase 2: 주요 글로벌 거래소 (2-4주)
**목표: Top 5 거래소 구현**

#### 2.1 Binance (14,411줄)
- [ ] Spot API
- [ ] USDM Futures
- [ ] COIN-M Futures
- [ ] 마진 거래

#### 2.2 OKX (9,178줄)
- [ ] Spot API
- [ ] Swap/Futures
- [ ] 옵션

#### 2.3 Bybit (9,577줄)
- [ ] Spot API
- [ ] Linear/Inverse Perpetual
- [ ] USDC Options

#### 2.4 Coinbase (5,270줄)
- [ ] Spot API
- [ ] Advanced Trade API

#### 2.5 Kraken (3,654줄)
- [ ] Spot API
- [ ] Futures (별도)

### Phase 3: WebSocket 지원 (2-3주)
**목표: 실시간 데이터 스트리밍**

#### 3.1 WebSocket 인프라
- [ ] WebSocket 클라이언트 구현
- [ ] 연결 관리 (재연결, 하트비트)
- [ ] 메시지 직렬화/역직렬화

#### 3.2 기본 Watch 메서드
- [ ] watchTicker / watchTickers
- [ ] watchOrderBook
- [ ] watchTrades
- [ ] watchOHLCV

#### 3.3 비공개 Watch 메서드
- [ ] watchBalance
- [ ] watchOrders
- [ ] watchMyTrades
- [ ] watchPositions

### Phase 4: 추가 거래소 (4-8주)
**목표: 나머지 95개 거래소**

#### 4.1 중형 거래소 (20개)
Gate, HTX, Bitget, KuCoin, MEXC, BitMart, Phemex, XT, BingX, CoinEx 등

#### 4.2 소형 거래소 (75개)
나머지 거래소들

---

## 6. 예상 작업량

### 코드 라인 추정
| Phase | 예상 Rust 코드 |
|-------|---------------|
| Phase 1 | ~5,000줄 |
| Phase 2 | ~30,000줄 |
| Phase 3 | ~10,000줄 |
| Phase 4 | ~100,000줄 |
| **Total** | **~145,000줄** |

### 시간 추정 (풀타임 기준)
| Phase | 예상 기간 |
|-------|----------|
| Phase 1 | 1-2주 |
| Phase 2 | 2-4주 |
| Phase 3 | 2-3주 |
| Phase 4 | 4-8주 |
| **Total** | **9-17주** |

---

## 7. 우선순위 결정 기준

### 거래소 선정 기준
1. **거래량** - 글로벌 거래량 상위
2. **기능 복잡도** - 선물, 마진, 옵션 지원
3. **사용자 요청** - 커뮤니티 수요
4. **코드 재사용** - 유사 거래소 그룹

### 기능 선정 기준
1. **필수 기능** - 시세, 주문, 잔고
2. **거래 기능** - 선물, 마진
3. **고급 기능** - 옵션, 그릭스
4. **부가 기능** - 입출금, 전송

---

## 8. 다음 단계

현재 세션에서 진행할 작업:

1. **Phase 1.1** - Precise 클래스 구현
2. **Phase 1.2** - Position, Leverage 타입 추가
3. **Phase 1.3** - 에러 타입 완성
4. **Phase 1.5** - 한국 거래소 비공개 API

시작하시겠습니까?
