//! Exchange trait and related types

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use shared_types::common::Exchange as ExchangeId;
use shared_types::error::TradingResult;
use shared_types::market::{Candle, Timeframe};
use shared_types::trading::{Order, OrderSide, OrderType};

/// 거래소 통합 인터페이스
///
/// 모든 거래소 구현체가 구현해야 하는 trait
#[async_trait]
pub trait Exchange: Send + Sync {
    /// 거래소 ID 반환
    fn id(&self) -> ExchangeId;

    /// 거래소 이름 반환
    fn name(&self) -> &str;

    // === Market Data ===

    /// OHLCV 캔들 데이터 조회
    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<DateTime<Utc>>,
        limit: Option<u32>,
    ) -> TradingResult<Vec<Candle>>;

    /// 현재 가격 조회
    async fn fetch_ticker(&self, symbol: &str) -> TradingResult<Ticker>;

    /// 호가창 조회
    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> TradingResult<OrderBook>;

    // === Trading ===

    /// 주문 생성
    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> TradingResult<Order>;

    /// 주문 취소
    async fn cancel_order(&self, order_id: &str, symbol: &str) -> TradingResult<Order>;

    /// 주문 조회
    async fn fetch_order(&self, order_id: &str, symbol: &str) -> TradingResult<Order>;

    /// 미체결 주문 목록
    async fn fetch_open_orders(&self, symbol: Option<&str>) -> TradingResult<Vec<Order>>;

    // === Account ===

    /// 잔고 조회
    async fn fetch_balance(&self) -> TradingResult<Balance>;

    // === Metadata ===

    /// 지원하는 마켓 목록
    async fn fetch_markets(&self) -> TradingResult<Vec<Market>>;

    /// 거래소 지원 기능 확인
    fn has(&self, feature: &str) -> bool;
}

/// 시세 정보
#[derive(Debug, Clone)]
pub struct Ticker {
    pub symbol: String,
    pub timestamp: DateTime<Utc>,
    pub last: Decimal,
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
    pub high: Option<Decimal>,
    pub low: Option<Decimal>,
    pub volume: Option<Decimal>,
    pub change: Option<Decimal>,
    pub percentage: Option<Decimal>,
}

/// 호가창
#[derive(Debug, Clone)]
pub struct OrderBook {
    pub symbol: String,
    pub timestamp: DateTime<Utc>,
    pub bids: Vec<(Decimal, Decimal)>, // (price, amount)
    pub asks: Vec<(Decimal, Decimal)>, // (price, amount)
}

/// 잔고 정보
#[derive(Debug, Clone)]
pub struct Balance {
    pub total: std::collections::HashMap<String, Decimal>,
    pub free: std::collections::HashMap<String, Decimal>,
    pub used: std::collections::HashMap<String, Decimal>,
}

/// 마켓 정보
#[derive(Debug, Clone)]
pub struct Market {
    pub id: String,
    pub symbol: String,
    pub base: String,
    pub quote: String,
    pub active: bool,
    pub precision: MarketPrecision,
    pub limits: MarketLimits,
}

/// 마켓 정밀도
#[derive(Debug, Clone)]
pub struct MarketPrecision {
    pub amount: u32,
    pub price: u32,
}

/// 마켓 제한
#[derive(Debug, Clone)]
pub struct MarketLimits {
    pub amount_min: Option<Decimal>,
    pub amount_max: Option<Decimal>,
    pub price_min: Option<Decimal>,
    pub price_max: Option<Decimal>,
    pub cost_min: Option<Decimal>,
}
