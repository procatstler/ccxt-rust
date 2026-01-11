//! KuCoin Futures Exchange Implementation
//!
//! Alias for KucoinFutures (lowercase naming variant)

use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::client::ExchangeConfig;
use crate::errors::CcxtResult;
use crate::types::{
    Balances, Currency, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, FundingRate,
    FundingRateHistory, Leverage, MarginMode, MarginModeInfo, Market, Order, OrderBook, OrderSide,
    OrderType, Position, SignedRequest, Ticker, Timeframe, Trade, Transaction, OHLCV,
};

use super::kucoin_futures::KucoinFutures;

/// KuCoin Futures 거래소 (KucoinFutures 래퍼)
///
/// kucoinfutures는 KuCoin의 선물/영구 계약 거래소입니다.
/// - Swap: true (영구 계약)
/// - Future: true (정산 선물)
/// - API: <https://api-futures.kucoin.com>
pub struct Kucoinfutures(KucoinFutures);

impl Kucoinfutures {
    /// 새 Kucoinfutures 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        Ok(Self(KucoinFutures::new(config)?))
    }
}

#[async_trait]
impl Exchange for Kucoinfutures {
    fn id(&self) -> ExchangeId {
        // Uses the same ExchangeId as KucoinFutures
        ExchangeId::KucoinFutures
    }

    fn name(&self) -> &str {
        "KuCoin Futures"
    }

    fn version(&self) -> &str {
        self.0.version()
    }

    fn countries(&self) -> &[&str] {
        self.0.countries()
    }

    fn rate_limit(&self) -> u64 {
        self.0.rate_limit()
    }

    fn has(&self) -> &ExchangeFeatures {
        self.0.has()
    }

    fn urls(&self) -> &ExchangeUrls {
        self.0.urls()
    }

    fn timeframes(&self) -> &HashMap<Timeframe, String> {
        self.0.timeframes()
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        self.0.market_id(symbol)
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        self.0.symbol(market_id)
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        self.0.sign(path, api, method, params, headers, body)
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        self.0.load_markets(reload).await
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        self.0.fetch_markets().await
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, Currency>> {
        self.0.fetch_currencies().await
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.0.fetch_ticker(symbol).await
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.0.fetch_tickers(symbols).await
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.0.fetch_order_book(symbol, limit).await
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.0.fetch_trades(symbol, since, limit).await
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        self.0.fetch_ohlcv(symbol, timeframe, since, limit).await
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        self.0.fetch_balance().await
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        self.0
            .create_order(symbol, order_type, side, amount, price)
            .await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.0.cancel_order(id, symbol).await
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.0.fetch_order(id, symbol).await
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.0.fetch_open_orders(symbol, since, limit).await
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.0.fetch_closed_orders(symbol, since, limit).await
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.0.fetch_my_trades(symbol, since, limit).await
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        self.0.fetch_deposits(code, since, limit).await
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        self.0.fetch_withdrawals(code, since, limit).await
    }

    async fn fetch_position(&self, symbol: &str) -> CcxtResult<Position> {
        self.0.fetch_position(symbol).await
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        self.0.fetch_positions(symbols).await
    }

    async fn fetch_positions_history(
        &self,
        symbols: Option<&[&str]>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Position>> {
        self.0.fetch_positions_history(symbols, since, limit).await
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        self.0.fetch_funding_rate(symbol).await
    }

    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        self.0
            .fetch_funding_rate_history(symbol, since, limit)
            .await
    }

    async fn set_margin_mode(
        &self,
        margin_mode: MarginMode,
        symbol: &str,
    ) -> CcxtResult<MarginModeInfo> {
        self.0.set_margin_mode(margin_mode, symbol).await
    }

    async fn add_margin(
        &self,
        symbol: &str,
        amount: Decimal,
    ) -> CcxtResult<crate::types::MarginModification> {
        self.0.add_margin(symbol, amount).await
    }

    async fn fetch_leverage(&self, symbol: &str) -> CcxtResult<Leverage> {
        self.0.fetch_leverage(symbol).await
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        self.0.fetch_deposit_address(code, network).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Kucoinfutures::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::KucoinFutures);
        assert_eq!(exchange.name(), "KuCoin Futures");
        assert_eq!(exchange.version(), "v1");

        // KuCoin Futures features
        assert!(exchange.has().swap);
        assert!(exchange.has().future);
        assert!(!exchange.has().spot);
        assert!(exchange.has().fetch_markets);
        assert!(exchange.has().fetch_ticker);
        assert!(exchange.has().fetch_positions);
        assert!(exchange.has().fetch_funding_rate);
        assert!(exchange.has().create_order);
    }

    #[test]
    fn test_rate_limit() {
        let config = ExchangeConfig::default();
        let exchange = Kucoinfutures::new(config).unwrap();

        assert_eq!(exchange.rate_limit(), 75);
    }

    #[test]
    fn test_countries() {
        let config = ExchangeConfig::default();
        let exchange = Kucoinfutures::new(config).unwrap();

        let countries = exchange.countries();
        assert_eq!(countries, &["SC"]);
    }
}
