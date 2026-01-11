//! Spot Trading API Trait
//!
//! Core trading operations for spot markets.

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::errors::CcxtResult;
use crate::not_supported;
use crate::types::{Balances, Order, OrderSide, OrderType, Trade};

/// Spot trading API
///
/// Core trading operations:
/// - Balance management
/// - Order creation (market, limit, stop orders)
/// - Order cancellation and modification
/// - Order and trade queries
#[async_trait]
pub trait SpotTradingApi: Send + Sync {
    // ========================================================================
    // Balance
    // ========================================================================

    /// Fetch account balances
    async fn fetch_balance(&self) -> CcxtResult<Balances>;

    // ========================================================================
    // Order Creation
    // ========================================================================

    /// Create an order
    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order>;

    /// Create a limit order
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

    /// Create a market order
    async fn create_market_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
    ) -> CcxtResult<Order> {
        self.create_order(symbol, OrderType::Market, side, amount, None)
            .await
    }

    /// Create multiple orders at once
    async fn create_orders(&self, orders: Vec<crate::types::OrderRequest>) -> CcxtResult<Vec<Order>> {
        let _ = orders;
        not_supported!("createOrders")
    }

    // ========================================================================
    // Stop Orders
    // ========================================================================

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
        not_supported!("createStopOrder")
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
        self.create_stop_order(
            symbol,
            OrderType::StopLimit,
            side,
            amount,
            Some(price),
            stop_price,
        )
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
        self.create_stop_order(
            symbol,
            OrderType::StopMarket,
            side,
            amount,
            None,
            stop_price,
        )
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
        not_supported!("createTakeProfitOrder")
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
        not_supported!("createStopLossOrder")
    }

    // ========================================================================
    // Order Cancellation
    // ========================================================================

    /// Cancel an order
    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;

    /// Cancel multiple orders
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
        not_supported!("cancelAllOrders")
    }

    // ========================================================================
    // Order Modification
    // ========================================================================

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
        not_supported!("editOrder")
    }

    // ========================================================================
    // Order Queries
    // ========================================================================

    /// Fetch a specific order
    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order>;

    /// Fetch open orders
    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>>;

    /// Fetch closed orders
    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchClosedOrders")
    }

    /// Fetch canceled orders
    async fn fetch_canceled_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchCanceledOrders")
    }

    /// Fetch all orders (open + closed + canceled)
    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchOrders")
    }

    // ========================================================================
    // Trade Queries
    // ========================================================================

    /// Fetch my trades (fills)
    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchMyTrades")
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
        not_supported!("fetchOrderTrades")
    }

    // ========================================================================
    // Trading Fees
    // ========================================================================

    /// Fetch trading fees for all symbols
    async fn fetch_trading_fees(&self) -> CcxtResult<std::collections::HashMap<String, crate::types::TradingFee>> {
        not_supported!("fetchTradingFees")
    }

    /// Fetch trading fee for a specific symbol
    async fn fetch_trading_fee(&self, symbol: &str) -> CcxtResult<crate::types::TradingFee> {
        let _ = symbol;
        not_supported!("fetchTradingFee")
    }
}
