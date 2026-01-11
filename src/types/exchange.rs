//! Exchange trait - Unified exchange interface
//!
//! Complete port of CCXT Exchange class to Rust

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::errors::CcxtResult;
use super::{
    Balances, Currency, Market, OHLCV, Order, OrderBook, OrderSide, OrderType,
    Ticker, Trade, Transaction,
    // Futures/Derivatives types
    Position, Leverage, LeverageTier, FundingRate, FundingRateHistory, OpenInterest, Liquidation,
    // Margin types
    BorrowInterest, CrossBorrowRate, IsolatedBorrowRate, MarginModeInfo, MarginLoan,
    margin::MarginModification,
    // Account types
    Account, DepositAddress, TransferEntry, LedgerEntry,
    // Position types
    MarginMode, PositionModeInfo,
    // WebSocket types
    WsMessage,
    // Derivatives types
    LongShortRatio,
    // Convert types
    ConvertCurrencyPair, ConvertQuote, ConvertTrade,
};

/// Exchange ID - identifies the exchange
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExchangeId {
    // Korean exchanges
    Upbit,
    Bithumb,
    Coinone,
    Korbit,
    // Major global exchanges
    Binance,
    BinanceCoinM,
    BinanceFutures,
    BinanceUs,
    Coinbase,
    CoinbaseAdvanced,
    CoinbaseExchange,
    CoinbaseInternational,
    Kraken,
    KrakenFutures,
    Okx,
    Bybit,
    Gate,
    Htx,
    Kucoin,
    KucoinFutures,
    Bitget,
    Mexc,
    Bitmart,
    Phemex,
    Bingx,
    Coinex,
    Timex,
    // Phase 16 additions
    Hyperliquid,
    Bitmex,
    Deribit,
    CryptoCom,
    Gemini,
    // Phase 17 additions - DEX and European
    Dydx,
    Bitstamp,
    Bitfinex,
    Whitebit,
    Lbank,
    Probit,
    P2b,
    Wavesexchange,
    // Japanese exchanges
    Bitflyer,
    Coincheck,
    Bitbank,
    Btcbox,
    Zaif,
    // APAC exchanges
    Btcmarkets,
    Indodax,
    Tokocrypto,
    Delta,
    Latoken,
    Independentreserve,
    // LATAM exchanges
    Foxbit,
    Mercado,
    Bitso,
    // European
    Bitvavo,
    // Additional exchanges
    Poloniex,
    Hitbtc,
    Ascendex,
    BlockchainCom,
    Cex,
    Exmo,
    Xt,
    // Stock and crypto brokers
    Alpaca,
    // Options platforms
    Blofin,
    // Additional WS implementations
    Woo,
    Cryptomus,
    // Asian exchanges
    Hashkey,
    Bitrue,
    Bigone,
    // African/Global exchanges
    Luno,
    // European exchanges
    Zonda,
    Bullish,
    Oceanex,
    Btcalpha,
    Coinmate,
    Coinmetro,
    Onetrading,
    // Korea-based exchanges
    Hollaex,
    // DEX platforms
    Paradex,
    // Solana-based exchanges
    Backpack,
    // Additional Phase 4 exchanges
    Yobit,
    Digifinex,
    Ndax,
    Btcturk,
    // Alias exchanges
    Bequant,    // Hitbtc alias
    MyOkx,      // OKX alias (EEA region)
    OkxUs,      // OKX alias (US region)
    Fmfwio,     // Hitbtc alias
    Gateio,     // Gate alias
    Huobi,      // HTX alias
    BinanceUsdm, // Binance USDⓈ-M Futures alias
    // Additional exchanges
    Toobit,
    // Australian exchanges
    Coinspot,
    // French/European exchanges
    Paymium,
    // Israeli exchanges
    Bit2c,
    // Indian exchanges
    Bitbns,
    Zebpay,
    // Taiwan exchanges
    Bitopro,
    // Philippine exchanges
    Coinsph,
    // Brazilian exchanges
    Novadax,
    // Global derivatives exchanges
    Deepcoin,
    // Japan exchanges (additional)
    Bittrade,
    // DEX exchanges
    Apex,
    // Derivatives exchanges
    Oxfun,
    // DEX exchanges (additional)
    Defx,
    // DEX derivatives
    Derive,
    // Crypto derivatives exchange
    CoinCatch,
}

impl ExchangeId {
    pub fn as_str(&self) -> &'static str {
        match self {
            // Korean
            ExchangeId::Upbit => "upbit",
            ExchangeId::Bithumb => "bithumb",
            ExchangeId::Coinone => "coinone",
            ExchangeId::Korbit => "korbit",
            // Global
            ExchangeId::Binance => "binance",
            ExchangeId::BinanceCoinM => "binancecoinm",
            ExchangeId::BinanceFutures => "binanceusdm",
            ExchangeId::BinanceUs => "binanceus",
            ExchangeId::Coinbase => "coinbase",
            ExchangeId::CoinbaseAdvanced => "coinbaseadvanced",
            ExchangeId::CoinbaseExchange => "coinbaseexchange",
            ExchangeId::CoinbaseInternational => "coinbaseinternational",
            ExchangeId::Kraken => "kraken",
            ExchangeId::KrakenFutures => "krakenfutures",
            ExchangeId::Okx => "okx",
            ExchangeId::Bybit => "bybit",
            ExchangeId::Gate => "gate",
            ExchangeId::Htx => "htx",
            ExchangeId::Kucoin => "kucoin",
            ExchangeId::KucoinFutures => "kucoinfutures",
            ExchangeId::Bitget => "bitget",
            ExchangeId::Mexc => "mexc",
            ExchangeId::Bitmart => "bitmart",
            ExchangeId::Phemex => "phemex",
            ExchangeId::Bingx => "bingx",
            ExchangeId::Coinex => "coinex",
            ExchangeId::Timex => "timex",
            // Phase 16 additions
            ExchangeId::Hyperliquid => "hyperliquid",
            ExchangeId::Bitmex => "bitmex",
            ExchangeId::Deribit => "deribit",
            ExchangeId::CryptoCom => "cryptocom",
            ExchangeId::Gemini => "gemini",
            // Phase 17 additions
            ExchangeId::Dydx => "dydx",
            ExchangeId::Bitstamp => "bitstamp",
            ExchangeId::Bitfinex => "bitfinex",
            ExchangeId::Whitebit => "whitebit",
            ExchangeId::Lbank => "lbank",
            ExchangeId::Probit => "probit",
            ExchangeId::P2b => "p2b",
            ExchangeId::Wavesexchange => "wavesexchange",
            ExchangeId::Bitflyer => "bitflyer",
            ExchangeId::Coincheck => "coincheck",
            ExchangeId::Bitbank => "bitbank",
            ExchangeId::Btcbox => "btcbox",
            ExchangeId::Zaif => "zaif",
            ExchangeId::Btcmarkets => "btcmarkets",
            ExchangeId::Indodax => "indodax",
            ExchangeId::Tokocrypto => "tokocrypto",
            ExchangeId::Delta => "delta",
            ExchangeId::Latoken => "latoken",
            ExchangeId::Independentreserve => "independentreserve",
            ExchangeId::Foxbit => "foxbit",
            ExchangeId::Mercado => "mercado",
            ExchangeId::Bitso => "bitso",
            ExchangeId::Bitvavo => "bitvavo",
            ExchangeId::Poloniex => "poloniex",
            ExchangeId::Hitbtc => "hitbtc",
            ExchangeId::Ascendex => "ascendex",
            ExchangeId::BlockchainCom => "blockchaincom",
            ExchangeId::Cex => "cex",
            ExchangeId::Exmo => "exmo",
            ExchangeId::Xt => "xt",
            ExchangeId::Alpaca => "alpaca",
            ExchangeId::Blofin => "blofin",
            ExchangeId::Woo => "woo",
            ExchangeId::Cryptomus => "cryptomus",
            ExchangeId::Hashkey => "hashkey",
            ExchangeId::Bitrue => "bitrue",
            ExchangeId::Bigone => "bigone",
            ExchangeId::Luno => "luno",
            ExchangeId::Zonda => "zonda",
            ExchangeId::Bullish => "bullish",
            ExchangeId::Oceanex => "oceanex",
            ExchangeId::Btcalpha => "btcalpha",
            ExchangeId::Coinmate => "coinmate",
            ExchangeId::Coinmetro => "coinmetro",
            ExchangeId::Onetrading => "onetrading",
            ExchangeId::Hollaex => "hollaex",
            ExchangeId::Paradex => "paradex",
            ExchangeId::Backpack => "backpack",
            ExchangeId::Yobit => "yobit",
            ExchangeId::Digifinex => "digifinex",
            ExchangeId::Ndax => "ndax",
            ExchangeId::Btcturk => "btcturk",
            // Alias exchanges
            ExchangeId::Bequant => "bequant",
            ExchangeId::MyOkx => "myokx",
            ExchangeId::OkxUs => "okxus",
            ExchangeId::Fmfwio => "fmfwio",
            ExchangeId::Gateio => "gateio",
            ExchangeId::Huobi => "huobi",
            ExchangeId::BinanceUsdm => "binanceusdm",
            ExchangeId::Toobit => "toobit",
            ExchangeId::Coinspot => "coinspot",
            ExchangeId::Paymium => "paymium",
            ExchangeId::Bit2c => "bit2c",
            ExchangeId::Bitbns => "bitbns",
            ExchangeId::Zebpay => "zebpay",
            ExchangeId::Bitopro => "bitopro",
            ExchangeId::Coinsph => "coinsph",
            ExchangeId::Novadax => "novadax",
            ExchangeId::Deepcoin => "deepcoin",
            ExchangeId::Bittrade => "bittrade",
            ExchangeId::Apex => "apex",
            ExchangeId::Oxfun => "oxfun",
            ExchangeId::Defx => "defx",
            ExchangeId::Derive => "derive",
            ExchangeId::CoinCatch => "coincatch",
        }
    }
}

impl std::fmt::Display for ExchangeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 타임프레임
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    #[serde(rename = "1s")]
    Second1,
    #[serde(rename = "1m")]
    Minute1,
    #[serde(rename = "3m")]
    Minute3,
    #[serde(rename = "5m")]
    Minute5,
    #[serde(rename = "15m")]
    Minute15,
    #[serde(rename = "30m")]
    Minute30,
    #[serde(rename = "1h")]
    Hour1,
    #[serde(rename = "2h")]
    Hour2,
    #[serde(rename = "3h")]
    Hour3,
    #[serde(rename = "4h")]
    Hour4,
    #[serde(rename = "6h")]
    Hour6,
    #[serde(rename = "8h")]
    Hour8,
    #[serde(rename = "12h")]
    Hour12,
    #[serde(rename = "1d")]
    Day1,
    #[serde(rename = "3d")]
    Day3,
    #[serde(rename = "1w")]
    Week1,
    #[serde(rename = "1M")]
    Month1,
}

impl Timeframe {
    pub fn as_str(&self) -> &'static str {
        match self {
            Timeframe::Second1 => "1s",
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour3 => "3h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Day3 => "3d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
        }
    }

    /// 밀리초 단위 기간
    pub fn to_millis(&self) -> i64 {
        match self {
            Timeframe::Second1 => 1000,
            Timeframe::Minute1 => 60 * 1000,
            Timeframe::Minute3 => 3 * 60 * 1000,
            Timeframe::Minute5 => 5 * 60 * 1000,
            Timeframe::Minute15 => 15 * 60 * 1000,
            Timeframe::Minute30 => 30 * 60 * 1000,
            Timeframe::Hour1 => 60 * 60 * 1000,
            Timeframe::Hour2 => 2 * 60 * 60 * 1000,
            Timeframe::Hour3 => 3 * 60 * 60 * 1000,
            Timeframe::Hour4 => 4 * 60 * 60 * 1000,
            Timeframe::Hour6 => 6 * 60 * 60 * 1000,
            Timeframe::Hour8 => 8 * 60 * 60 * 1000,
            Timeframe::Hour12 => 12 * 60 * 60 * 1000,
            Timeframe::Day1 => 24 * 60 * 60 * 1000,
            Timeframe::Day3 => 3 * 24 * 60 * 60 * 1000,
            Timeframe::Week1 => 7 * 24 * 60 * 60 * 1000,
            Timeframe::Month1 => 30 * 24 * 60 * 60 * 1000,
        }
    }
}

/// Exchange feature flags - indicates supported functionality
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeFeatures {
    // === Market Types ===
    pub cors: bool,
    pub spot: bool,
    pub margin: bool,
    pub swap: bool,
    pub future: bool,
    pub option: bool,

    // === Public API ===
    pub fetch_markets: bool,
    pub fetch_currencies: bool,
    pub fetch_ticker: bool,
    pub fetch_tickers: bool,
    pub fetch_order_book: bool,
    pub fetch_order_books: bool,
    pub fetch_trades: bool,
    pub fetch_ohlcv: bool,
    pub fetch_status: bool,
    pub fetch_time: bool,
    pub fetch_bids_asks: bool,
    pub fetch_last_prices: bool,

    // === Private Trading API ===
    pub fetch_balance: bool,
    pub create_order: bool,
    pub create_orders: bool,
    pub create_limit_order: bool,
    pub create_market_order: bool,
    pub create_stop_order: bool,
    pub create_stop_limit_order: bool,
    pub create_stop_market_order: bool,
    pub create_post_only_order: bool,
    pub create_trailing_amount_order: bool,
    pub create_trailing_percent_order: bool,
    pub edit_order: bool,
    pub cancel_order: bool,
    pub cancel_orders: bool,
    pub cancel_all_orders: bool,
    pub cancel_all_orders_after: bool,
    pub fetch_order: bool,
    pub fetch_orders: bool,
    pub fetch_open_orders: bool,
    pub fetch_closed_orders: bool,
    pub fetch_canceled_orders: bool,
    pub fetch_my_trades: bool,
    pub fetch_order_trades: bool,

    // === Account/Wallet ===
    pub fetch_accounts: bool,
    pub fetch_ledger: bool,
    pub fetch_deposits: bool,
    pub fetch_withdrawals: bool,
    pub fetch_deposits_withdrawals: bool,
    pub fetch_deposit_address: bool,
    pub fetch_deposit_addresses: bool,
    pub create_deposit_address: bool,
    pub withdraw: bool,
    pub transfer: bool,
    pub fetch_transfers: bool,

    // === Futures/Derivatives ===
    pub fetch_funding_rate: bool,
    pub fetch_funding_rates: bool,
    pub fetch_funding_rate_history: bool,
    pub fetch_funding_history: bool,
    pub fetch_open_interest: bool,
    pub fetch_open_interest_history: bool,
    pub fetch_liquidations: bool,
    pub fetch_my_liquidations: bool,
    pub fetch_mark_price: bool,
    pub fetch_mark_prices: bool,
    pub fetch_index_price: bool,
    pub fetch_mark_ohlcv: bool,
    pub fetch_index_ohlcv: bool,
    pub fetch_premium_index_ohlcv: bool,
    pub fetch_long_short_ratio: bool,
    pub fetch_long_short_ratio_history: bool,

    // === Positions ===
    pub fetch_position: bool,
    pub fetch_positions: bool,
    pub fetch_positions_risk: bool,
    pub fetch_position_history: bool,
    pub fetch_positions_history: bool,
    pub close_position: bool,
    pub close_all_positions: bool,
    pub set_position_mode: bool,
    pub fetch_position_mode: bool,

    // === Leverage ===
    pub set_leverage: bool,
    pub fetch_leverage: bool,
    pub fetch_leverages: bool,
    pub fetch_leverage_tiers: bool,
    pub fetch_market_leverage_tiers: bool,

    // === Margin ===
    pub set_margin_mode: bool,
    pub fetch_margin_mode: bool,
    pub fetch_margin_modes: bool,
    pub borrow_margin: bool,
    pub borrow_cross_margin: bool,
    pub borrow_isolated_margin: bool,
    pub repay_margin: bool,
    pub repay_cross_margin: bool,
    pub repay_isolated_margin: bool,
    pub fetch_borrow_rate: bool,
    pub fetch_borrow_rates: bool,
    pub fetch_borrow_interest: bool,
    pub add_margin: bool,
    pub reduce_margin: bool,
    pub set_margin: bool,
    pub fetch_margin_adjustment_history: bool,

    // === Trading Fees ===
    pub fetch_trading_fee: bool,
    pub fetch_trading_fees: bool,

    // === WebSocket ===
    pub ws: bool,
    pub watch_ticker: bool,
    pub watch_tickers: bool,
    pub watch_order_book: bool,
    pub watch_order_book_for_symbols: bool,
    pub watch_trades: bool,
    pub watch_trades_for_symbols: bool,
    pub watch_ohlcv: bool,
    pub watch_ohlcv_for_symbols: bool,
    pub watch_balance: bool,
    pub watch_orders: bool,
    pub watch_orders_for_symbols: bool,
    pub watch_my_trades: bool,
    pub watch_my_trades_for_symbols: bool,
    pub watch_positions: bool,
    pub watch_bids_asks: bool,
    pub watch_mark_price: bool,
    pub watch_funding_rate: bool,
    pub watch_liquidations: bool,

    // === Convert ===
    pub fetch_convert_currencies: bool,
    pub fetch_convert_quote: bool,
    pub create_convert_trade: bool,
    pub fetch_convert_trade: bool,
    pub fetch_convert_trade_history: bool,
}

/// 거래소 URL 정보
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExchangeUrls {
    pub logo: Option<String>,
    pub api: HashMap<String, String>,
    pub www: Option<String>,
    pub doc: Vec<String>,
    pub fees: Option<String>,
}

/// 서명된 요청
#[derive(Debug, Clone)]
pub struct SignedRequest {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

/// 거래소 통합 인터페이스
///
/// CCXT의 Exchange 클래스를 Rust trait으로 포팅
#[async_trait]
pub trait Exchange: Send + Sync {
    // === 메타데이터 ===

    /// 거래소 ID
    fn id(&self) -> ExchangeId;

    /// 거래소 이름
    fn name(&self) -> &str;

    /// API 버전
    fn version(&self) -> &str {
        "v1"
    }

    /// 국가 목록
    fn countries(&self) -> &[&str] {
        &[]
    }

    /// 레이트 리밋 (밀리초)
    fn rate_limit(&self) -> u64 {
        1000
    }

    /// 지원 기능
    fn has(&self) -> &ExchangeFeatures;

    /// 특정 기능 지원 여부
    fn has_feature(&self, feature: &str) -> bool {
        let features = self.has();
        match feature {
            "fetchMarkets" => features.fetch_markets,
            "fetchCurrencies" => features.fetch_currencies,
            "fetchTicker" => features.fetch_ticker,
            "fetchTickers" => features.fetch_tickers,
            "fetchOrderBook" => features.fetch_order_book,
            "fetchTrades" => features.fetch_trades,
            "fetchOHLCV" => features.fetch_ohlcv,
            "fetchBalance" => features.fetch_balance,
            "createOrder" => features.create_order,
            "cancelOrder" => features.cancel_order,
            "fetchOrder" => features.fetch_order,
            "fetchOrders" => features.fetch_orders,
            "fetchOpenOrders" => features.fetch_open_orders,
            "fetchClosedOrders" => features.fetch_closed_orders,
            "fetchMyTrades" => features.fetch_my_trades,
            "fetchDeposits" => features.fetch_deposits,
            "fetchWithdrawals" => features.fetch_withdrawals,
            "withdraw" => features.withdraw,
            _ => false,
        }
    }

    /// URL 정보
    fn urls(&self) -> &ExchangeUrls;

    /// 지원 타임프레임
    fn timeframes(&self) -> &HashMap<Timeframe, String>;

    // === Public API ===

    /// 마켓 로드 (캐싱)
    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>>;

    /// 마켓 목록 조회
    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>>;

    /// 화폐 목록 조회
    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, Currency>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchCurrencies".into(),
        })
    }

    /// 시세 조회
    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker>;

    /// 복수 시세 조회
    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchTickers".into(),
        })
    }

    /// 호가창 조회
    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook>;

    /// 체결 내역 조회
    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>>;

    /// OHLCV 조회
    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>>;

    // === Private API ===

    /// 잔고 조회
    async fn fetch_balance(&self) -> CcxtResult<Balances>;

    /// 주문 생성
    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order>;

    /// 지정가 주문 생성
    async fn create_limit_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
    ) -> CcxtResult<Order> {
        self.create_order(symbol, OrderType::Limit, side, amount, Some(price))
            .await
    }

    /// 시장가 주문 생성
    async fn create_market_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
    ) -> CcxtResult<Order> {
        self.create_order(symbol, OrderType::Market, side, amount, None)
            .await
    }

    /// 주문 취소
    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;

    /// 복수 주문 취소
    async fn cancel_orders(&self, ids: &[&str], symbol: &str) -> CcxtResult<Vec<Order>> {
        let mut results = Vec::new();
        for id in ids {
            results.push(self.cancel_order(id, symbol).await?);
        }
        Ok(results)
    }

    /// Cancel all orders for a symbol
    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "cancelAllOrders".into(),
        })
    }

    /// Edit/amend an existing order
    async fn edit_order(
        &self,
        id: &str,
        symbol: &str,
        order_type: Option<OrderType>,
        side: Option<OrderSide>,
        amount: Option<Decimal>,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let _ = (id, symbol, order_type, side, amount, price);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "editOrder".into(),
        })
    }

    /// 주문 조회
    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;

    /// 미체결 주문 목록
    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>>;

    /// 체결 완료 주문 목록
    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchClosedOrders".into(),
        })
    }

    /// 취소된 주문 목록
    async fn fetch_canceled_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchCanceledOrders".into(),
        })
    }

    /// 전체 주문 목록 (미체결 + 체결 + 취소)
    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchOrders".into(),
        })
    }

    /// 내 체결 내역
    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchMyTrades".into(),
        })
    }

    // === 입출금 ===

    /// 입금 내역
    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let _ = (code, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchDeposits".into(),
        })
    }

    /// 출금 내역
    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let _ = (code, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchWithdrawals".into(),
        })
    }

    /// Withdraw funds
    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        let _ = (code, amount, address, tag, network);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "withdraw".into(),
        })
    }

    /// Fetch deposit address
    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let _ = (code, network);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchDepositAddress".into(),
        })
    }

    /// Fetch deposit addresses for multiple currencies
    async fn fetch_deposit_addresses(
        &self,
        codes: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, DepositAddress>> {
        let _ = codes;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchDepositAddresses".into(),
        })
    }

    /// Fetch deposit addresses by network
    async fn fetch_deposit_addresses_by_network(
        &self,
        code: &str,
    ) -> CcxtResult<HashMap<String, DepositAddress>> {
        let _ = code;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchDepositAddressesByNetwork".into(),
        })
    }

    /// Create a new deposit address
    async fn create_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let _ = (code, network);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "createDepositAddress".into(),
        })
    }

    /// Transfer funds between accounts
    async fn transfer(
        &self,
        code: &str,
        amount: Decimal,
        from_account: &str,
        to_account: &str,
    ) -> CcxtResult<TransferEntry> {
        let _ = (code, amount, from_account, to_account);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "transfer".into(),
        })
    }

    /// Fetch transfer history
    async fn fetch_transfers(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<TransferEntry>> {
        let _ = (code, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchTransfers".into(),
        })
    }

    /// Fetch account ledger
    async fn fetch_ledger(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<LedgerEntry>> {
        let _ = (code, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchLedger".into(),
        })
    }

    /// Fetch accounts
    async fn fetch_accounts(&self) -> CcxtResult<Vec<Account>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchAccounts".into(),
        })
    }

    // === Positions (Futures/Derivatives) ===

    /// Fetch single position
    async fn fetch_position(
        &self,
        symbol: &str,
    ) -> CcxtResult<Position> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchPosition".into(),
        })
    }

    /// Fetch all positions
    async fn fetch_positions(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<Vec<Position>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchPositions".into(),
        })
    }

    /// Close a position
    async fn close_position(
        &self,
        symbol: &str,
        side: Option<OrderSide>,
    ) -> CcxtResult<Order> {
        let _ = (symbol, side);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "closePosition".into(),
        })
    }

    /// Close all positions
    async fn close_all_positions(&self) -> CcxtResult<Vec<Order>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "closeAllPositions".into(),
        })
    }

    /// Fetch position history for a symbol
    async fn fetch_position_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Position>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchPositionHistory".into(),
        })
    }

    /// Fetch all position history
    async fn fetch_positions_history(
        &self,
        symbols: Option<&[&str]>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Position>> {
        let _ = (symbols, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchPositionsHistory".into(),
        })
    }

    // === Position Mode ===

    /// Set position mode (hedged/one-way)
    async fn set_position_mode(
        &self,
        hedged: bool,
        symbol: Option<&str>,
    ) -> CcxtResult<PositionModeInfo> {
        let _ = (hedged, symbol);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "setPositionMode".into(),
        })
    }

    /// Fetch current position mode
    async fn fetch_position_mode(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<PositionModeInfo> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchPositionMode".into(),
        })
    }

    // === Leverage ===

    /// Set leverage for a symbol
    async fn set_leverage(
        &self,
        leverage: Decimal,
        symbol: &str,
    ) -> CcxtResult<Leverage> {
        let _ = (leverage, symbol);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "setLeverage".into(),
        })
    }

    /// Fetch current leverage for a symbol
    async fn fetch_leverage(
        &self,
        symbol: &str,
    ) -> CcxtResult<Leverage> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchLeverage".into(),
        })
    }

    /// Fetch leverage tiers for a symbol
    async fn fetch_leverage_tiers(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Vec<LeverageTier>>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchLeverageTiers".into(),
        })
    }

    // === Margin Mode ===

    /// Set margin mode (isolated/cross)
    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        let _ = (margin_mode, symbol);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "setMarginMode".into(),
        })
    }

    /// Fetch current margin mode
    async fn fetch_margin_mode(
        &self,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchMarginMode".into(),
        })
    }

    // === Margin Operations ===

    /// Borrow margin
    async fn borrow_margin(
        &self,
        code: &str,
        amount: Decimal,
        symbol: Option<&str>,
    ) -> CcxtResult<BorrowInterest> {
        let _ = (code, amount, symbol);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "borrowMargin".into(),
        })
    }

    /// Repay margin
    async fn repay_margin(
        &self,
        code: &str,
        amount: Decimal,
        symbol: Option<&str>,
    ) -> CcxtResult<BorrowInterest> {
        let _ = (code, amount, symbol);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "repayMargin".into(),
        })
    }

    /// Borrow cross margin
    async fn borrow_cross_margin(
        &self,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginLoan> {
        let _ = (code, amount);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "borrowCrossMargin".into(),
        })
    }

    /// Borrow isolated margin
    async fn borrow_isolated_margin(
        &self,
        symbol: &str,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginLoan> {
        let _ = (symbol, code, amount);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "borrowIsolatedMargin".into(),
        })
    }

    /// Repay cross margin
    async fn repay_cross_margin(
        &self,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginLoan> {
        let _ = (code, amount);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "repayCrossMargin".into(),
        })
    }

    /// Repay isolated margin
    async fn repay_isolated_margin(
        &self,
        symbol: &str,
        code: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginLoan> {
        let _ = (symbol, code, amount);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "repayIsolatedMargin".into(),
        })
    }

    /// Add margin to a position
    async fn add_margin(
        &self,
        symbol: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginModification> {
        let _ = (symbol, amount);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "addMargin".into(),
        })
    }

    /// Reduce margin from a position
    async fn reduce_margin(
        &self,
        symbol: &str,
        amount: Decimal,
    ) -> CcxtResult<MarginModification> {
        let _ = (symbol, amount);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "reduceMargin".into(),
        })
    }

    /// Fetch borrow rate for cross margin
    async fn fetch_cross_borrow_rate(
        &self,
        code: &str,
    ) -> CcxtResult<CrossBorrowRate> {
        let _ = code;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchCrossBorrowRate".into(),
        })
    }

    /// Fetch borrow rate for isolated margin
    async fn fetch_isolated_borrow_rate(
        &self,
        symbol: &str,
    ) -> CcxtResult<IsolatedBorrowRate> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchIsolatedBorrowRate".into(),
        })
    }

    /// Fetch borrow interest
    async fn fetch_borrow_interest(
        &self,
        code: Option<&str>,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<BorrowInterest>> {
        let _ = (code, symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchBorrowInterest".into(),
        })
    }

    // === Funding Rates ===

    /// Fetch current funding rate
    async fn fetch_funding_rate(
        &self,
        symbol: &str,
    ) -> CcxtResult<FundingRate> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchFundingRate".into(),
        })
    }

    /// Fetch funding rates for multiple symbols
    async fn fetch_funding_rates(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, FundingRate>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchFundingRates".into(),
        })
    }

    /// Fetch funding rate history
    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchFundingRateHistory".into(),
        })
    }

    /// Fetch funding history (payments received/paid)
    async fn fetch_funding_history(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<super::FundingHistory>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchFundingHistory".into(),
        })
    }

    // === Mark Price / Index Price ===

    /// Fetch mark price for a symbol (futures/perpetual)
    async fn fetch_mark_price(
        &self,
        symbol: &str,
    ) -> CcxtResult<Ticker> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchMarkPrice".into(),
        })
    }

    /// Fetch mark prices for multiple symbols
    async fn fetch_mark_prices(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchMarkPrices".into(),
        })
    }

    /// Fetch index price for a symbol
    async fn fetch_index_price(
        &self,
        symbol: &str,
    ) -> CcxtResult<Ticker> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchIndexPrice".into(),
        })
    }

    // === Open Interest ===

    /// Fetch open interest for a symbol
    async fn fetch_open_interest(
        &self,
        symbol: &str,
    ) -> CcxtResult<OpenInterest> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchOpenInterest".into(),
        })
    }

    /// Fetch open interest history
    async fn fetch_open_interest_history(
        &self,
        symbol: &str,
        timeframe: Option<Timeframe>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OpenInterest>> {
        let _ = (symbol, timeframe, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchOpenInterestHistory".into(),
        })
    }

    // === Liquidations ===

    /// Fetch liquidations
    async fn fetch_liquidations(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Liquidation>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchLiquidations".into(),
        })
    }

    /// Fetch my liquidations
    async fn fetch_my_liquidations(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Liquidation>> {
        let _ = (symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchMyLiquidations".into(),
        })
    }

    // === Long/Short Ratio ===

    /// Fetch long/short ratio for a symbol
    async fn fetch_long_short_ratio(
        &self,
        symbol: &str,
        timeframe: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<LongShortRatio>> {
        let _ = (symbol, timeframe, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchLongShortRatio".into(),
        })
    }

    // === Mark/Index OHLCV ===

    /// Fetch mark price OHLCV (candles)
    async fn fetch_mark_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let _ = (symbol, timeframe, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchMarkOHLCV".into(),
        })
    }

    /// Fetch index price OHLCV (candles)
    async fn fetch_index_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let _ = (symbol, timeframe, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchIndexOHLCV".into(),
        })
    }

    // === Advanced Order Management (Phase 5) ===

    /// Create multiple orders at once
    async fn create_orders(
        &self,
        orders: Vec<super::OrderRequest>,
    ) -> CcxtResult<Vec<Order>> {
        let _ = orders;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "createOrders".into(),
        })
    }

    /// Fetch trades for a specific order
    async fn fetch_order_trades(
        &self,
        id: &str,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let _ = (id, symbol, since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchOrderTrades".into(),
        })
    }

    // === Fee APIs (Phase 5) ===

    /// Fetch trading fees for all symbols
    async fn fetch_trading_fees(&self) -> CcxtResult<HashMap<String, super::TradingFee>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchTradingFees".into(),
        })
    }

    /// Fetch trading fee for a specific symbol
    async fn fetch_trading_fee(&self, symbol: &str) -> CcxtResult<super::TradingFee> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchTradingFee".into(),
        })
    }

    /// Fetch deposit and withdrawal fees
    async fn fetch_deposit_withdraw_fees(
        &self,
        codes: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, super::DepositWithdrawFee>> {
        let _ = codes;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchDepositWithdrawFees".into(),
        })
    }

    // === Market Data APIs (Phase 5) ===

    /// Fetch server time
    async fn fetch_time(&self) -> CcxtResult<i64> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchTime".into(),
        })
    }

    /// Fetch exchange status
    async fn fetch_status(&self) -> CcxtResult<super::ExchangeStatus> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchStatus".into(),
        })
    }

    /// Fetch best bid/ask for symbols
    async fn fetch_bids_asks(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, super::BidAsk>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchBidsAsks".into(),
        })
    }

    // === Stop Orders (Phase 8) ===

    /// Create a stop order
    async fn create_stop_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
        stop_price: Decimal,
    ) -> CcxtResult<Order> {
        let _ = (symbol, order_type, side, amount, price, stop_price);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "createStopOrder".into(),
        })
    }

    /// Create a stop-limit order
    async fn create_stop_limit_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
        stop_price: Decimal,
    ) -> CcxtResult<Order> {
        self.create_stop_order(symbol, OrderType::StopLimit, side, amount, Some(price), stop_price)
            .await
    }

    /// Create a stop-market order
    async fn create_stop_market_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        stop_price: Decimal,
    ) -> CcxtResult<Order> {
        self.create_stop_order(symbol, OrderType::StopMarket, side, amount, None, stop_price)
            .await
    }

    /// Create a take-profit order
    async fn create_take_profit_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
        take_profit_price: Decimal,
    ) -> CcxtResult<Order> {
        let _ = (symbol, order_type, side, amount, price, take_profit_price);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "createTakeProfitOrder".into(),
        })
    }

    /// Create a stop-loss order
    async fn create_stop_loss_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
        stop_loss_price: Decimal,
    ) -> CcxtResult<Order> {
        let _ = (symbol, order_type, side, amount, price, stop_loss_price);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "createStopLossOrder".into(),
        })
    }

    // === WebSocket API (Real-time Data) ===

    /// Watch ticker updates for a symbol
    async fn watch_ticker(
        &self,
        symbol: &str,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchTicker".into(),
        })
    }

    /// Watch ticker updates for multiple symbols
    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchTickers".into(),
        })
    }

    /// Watch order book updates for a symbol
    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = (symbol, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOrderBook".into(),
        })
    }

    /// Watch order book updates for multiple symbols
    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        limit: Option<u32>,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = (symbols, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOrderBookForSymbols".into(),
        })
    }

    /// Watch trade updates for a symbol
    async fn watch_trades(
        &self,
        symbol: &str,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchTrades".into(),
        })
    }

    /// Watch trade updates for multiple symbols
    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchTradesForSymbols".into(),
        })
    }

    /// Watch OHLCV candle updates for a symbol
    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = (symbol, timeframe);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOHLCV".into(),
        })
    }

    /// Watch OHLCV candle updates for multiple symbols
    async fn watch_ohlcv_for_symbols(
        &self,
        symbols: &[&str],
        timeframe: Timeframe,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = (symbols, timeframe);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOHLCVForSymbols".into(),
        })
    }

    /// Watch balance updates (requires authentication)
    async fn watch_balance(
        &self,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchBalance".into(),
        })
    }

    /// Watch order updates (requires authentication)
    async fn watch_orders(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOrders".into(),
        })
    }

    /// Watch order updates for multiple symbols (requires authentication)
    async fn watch_orders_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOrdersForSymbols".into(),
        })
    }

    /// Watch my trade updates (requires authentication)
    async fn watch_my_trades(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchMyTrades".into(),
        })
    }

    /// Watch my trade updates for multiple symbols (requires authentication)
    async fn watch_my_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchMyTradesForSymbols".into(),
        })
    }

    /// Watch position updates (requires authentication)
    async fn watch_positions(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchPositions".into(),
        })
    }

    // === Convert ===

    /// Fetch supported currency pairs for conversion
    async fn fetch_convert_currencies(&self) -> CcxtResult<Vec<ConvertCurrencyPair>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchConvertCurrencies".into(),
        })
    }

    /// Fetch a quote for currency conversion
    async fn fetch_convert_quote(
        &self,
        from_code: &str,
        to_code: &str,
        amount: Decimal,
    ) -> CcxtResult<ConvertQuote> {
        let _ = (from_code, to_code, amount);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchConvertQuote".into(),
        })
    }

    /// Create a convert trade (execute conversion)
    async fn create_convert_trade(
        &self,
        quote_id: &str,
    ) -> CcxtResult<ConvertTrade> {
        let _ = quote_id;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "createConvertTrade".into(),
        })
    }

    /// Fetch a specific convert trade
    async fn fetch_convert_trade(
        &self,
        id: &str,
    ) -> CcxtResult<ConvertTrade> {
        let _ = id;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchConvertTrade".into(),
        })
    }

    /// Fetch convert trade history
    async fn fetch_convert_trade_history(
        &self,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<ConvertTrade>> {
        let _ = (since, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "fetchConvertTradeHistory".into(),
        })
    }

    // === Utilities ===

    /// Convert symbol to market ID
    fn market_id(&self, symbol: &str) -> Option<String>;

    /// Convert market ID to symbol
    fn symbol(&self, market_id: &str) -> Option<String>;

    /// Sign a request
    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeframe() {
        assert_eq!(Timeframe::Minute1.as_str(), "1m");
        assert_eq!(Timeframe::Hour1.to_millis(), 3600000);
        assert_eq!(Timeframe::Day1.to_millis(), 86400000);
    }

    #[test]
    fn test_exchange_id() {
        assert_eq!(ExchangeId::Upbit.as_str(), "upbit");
        assert_eq!(format!("{}", ExchangeId::Bithumb), "bithumb");
    }
}
