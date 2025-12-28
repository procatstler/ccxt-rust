//! Order type - 주문 정보

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{Fee, Trade};

/// 주문 상태
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderStatus {
    Open,
    Closed,
    Canceled,
    Expired,
    Rejected,
}

/// 주문 측면
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    Buy,
    Sell,
}

/// 주문 타입
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    Limit,
    Market,
    StopLimit,
    StopMarket,
    StopLoss,
    StopLossLimit,
    TakeProfit,
    TakeProfitLimit,
    TakeProfitMarket,
    LimitMaker,
    TrailingStopMarket,
}

/// 주문 유효 기간
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    GTC, // Good Till Canceled
    GTT, // Good Till Time
    IOC, // Immediate Or Cancel
    FOK, // Fill Or Kill
    PO,  // Post Only
}

/// 주문 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    /// 주문 ID
    pub id: String,
    /// 클라이언트 주문 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,
    /// 타임스탬프 (밀리초)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// ISO 8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
    /// 최종 체결 타임스탬프
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_trade_timestamp: Option<i64>,
    /// 최종 업데이트 타임스탬프
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_timestamp: Option<i64>,
    /// 주문 상태
    pub status: OrderStatus,
    /// 심볼
    pub symbol: String,
    /// 주문 타입
    #[serde(rename = "type")]
    pub order_type: OrderType,
    /// 유효 기간
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_in_force: Option<TimeInForce>,
    /// 매수/매도
    pub side: OrderSide,
    /// 주문 가격
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<Decimal>,
    /// 평균 체결가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average: Option<Decimal>,
    /// 주문 수량
    pub amount: Decimal,
    /// 체결된 수량
    #[serde(default)]
    pub filled: Decimal,
    /// 미체결 수량
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<Decimal>,
    /// 손절가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_price: Option<Decimal>,
    /// 트리거가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_price: Option<Decimal>,
    /// 이익실현가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take_profit_price: Option<Decimal>,
    /// 손절가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_loss_price: Option<Decimal>,
    /// 총 비용 (quote 화폐)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Decimal>,
    /// 체결 내역
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trades: Vec<Trade>,
    /// 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Fee>,
    /// 수수료 목록
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fees: Vec<Fee>,
    /// 포지션 감소만 (파생상품)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_only: Option<bool>,
    /// 메이커 전용
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_only: Option<bool>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

impl Order {
    /// 새 주문 생성
    pub fn new(
        id: String,
        symbol: String,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
    ) -> Self {
        Self {
            id,
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price: None,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::Value::Null,
        }
    }

    /// 지정가 주문 생성
    pub fn limit(
        id: String,
        symbol: String,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
    ) -> Self {
        let mut order = Self::new(id, symbol, OrderType::Limit, side, amount);
        order.price = Some(price);
        order
    }

    /// 시장가 주문 생성
    pub fn market(id: String, symbol: String, side: OrderSide, amount: Decimal) -> Self {
        Self::new(id, symbol, OrderType::Market, side, amount)
    }

    /// 타임스탬프 설정
    pub fn with_timestamp(mut self, ts: i64) -> Self {
        self.timestamp = Some(ts);
        self.datetime = Some(
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        self
    }

    /// 상태 설정
    pub fn with_status(mut self, status: OrderStatus) -> Self {
        self.status = status;
        self
    }

    /// 체결됨 여부
    pub fn is_filled(&self) -> bool {
        self.status == OrderStatus::Closed && self.filled >= self.amount
    }

    /// 미체결 여부
    pub fn is_open(&self) -> bool {
        self.status == OrderStatus::Open
    }

    /// 취소됨 여부
    pub fn is_canceled(&self) -> bool {
        self.status == OrderStatus::Canceled
    }

    /// 체결률 (%)
    pub fn fill_percentage(&self) -> Decimal {
        if self.amount > Decimal::ZERO {
            self.filled / self.amount * Decimal::ONE_HUNDRED
        } else {
            Decimal::ZERO
        }
    }

    /// 미체결 수량 계산
    pub fn calculate_remaining(&self) -> Decimal {
        self.amount - self.filled
    }
}

impl Default for Order {
    fn default() -> Self {
        Self::new(
            String::new(),
            String::new(),
            OrderType::Limit,
            OrderSide::Buy,
            Decimal::ZERO,
        )
    }
}

/// 주문 요청 - 복수 주문 생성시 사용
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderRequest {
    /// 심볼 (예: "BTC/USDT")
    pub symbol: String,
    /// 주문 타입
    #[serde(rename = "type")]
    pub order_type: OrderType,
    /// 매수/매도
    pub side: OrderSide,
    /// 주문 수량
    pub amount: Decimal,
    /// 주문 가격 (지정가 주문시)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<Decimal>,
    /// 손절가 (스탑 주문시)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_price: Option<Decimal>,
    /// 이익실현가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take_profit_price: Option<Decimal>,
    /// 손절가 (TP/SL 주문시)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_loss_price: Option<Decimal>,
    /// 클라이언트 주문 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,
    /// 유효 기간
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_in_force: Option<TimeInForce>,
    /// 포지션 감소만 (파생상품)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_only: Option<bool>,
    /// 메이커 전용
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_only: Option<bool>,
    /// 추가 파라미터
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<std::collections::HashMap<String, serde_json::Value>>,
}

impl OrderRequest {
    /// 새 주문 요청 생성
    pub fn new(symbol: &str, order_type: OrderType, side: OrderSide, amount: Decimal) -> Self {
        Self {
            symbol: symbol.to_string(),
            order_type,
            side,
            amount,
            price: None,
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            client_order_id: None,
            time_in_force: None,
            reduce_only: None,
            post_only: None,
            params: None,
        }
    }

    /// 지정가 주문 요청
    pub fn limit(symbol: &str, side: OrderSide, amount: Decimal, price: Decimal) -> Self {
        let mut req = Self::new(symbol, OrderType::Limit, side, amount);
        req.price = Some(price);
        req
    }

    /// 시장가 주문 요청
    pub fn market(symbol: &str, side: OrderSide, amount: Decimal) -> Self {
        Self::new(symbol, OrderType::Market, side, amount)
    }

    /// 스탑 리밋 주문 요청
    pub fn stop_limit(
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
        stop_price: Decimal,
    ) -> Self {
        let mut req = Self::new(symbol, OrderType::StopLimit, side, amount);
        req.price = Some(price);
        req.stop_price = Some(stop_price);
        req
    }

    /// 스탑 마켓 주문 요청
    pub fn stop_market(symbol: &str, side: OrderSide, amount: Decimal, stop_price: Decimal) -> Self {
        let mut req = Self::new(symbol, OrderType::StopMarket, side, amount);
        req.stop_price = Some(stop_price);
        req
    }

    /// 가격 설정
    pub fn with_price(mut self, price: Decimal) -> Self {
        self.price = Some(price);
        self
    }

    /// 스탑 가격 설정
    pub fn with_stop_price(mut self, stop_price: Decimal) -> Self {
        self.stop_price = Some(stop_price);
        self
    }

    /// Take Profit 가격 설정
    pub fn with_take_profit(mut self, take_profit_price: Decimal) -> Self {
        self.take_profit_price = Some(take_profit_price);
        self
    }

    /// Stop Loss 가격 설정
    pub fn with_stop_loss(mut self, stop_loss_price: Decimal) -> Self {
        self.stop_loss_price = Some(stop_loss_price);
        self
    }

    /// 클라이언트 주문 ID 설정
    pub fn with_client_order_id(mut self, client_order_id: &str) -> Self {
        self.client_order_id = Some(client_order_id.to_string());
        self
    }

    /// 유효 기간 설정
    pub fn with_time_in_force(mut self, tif: TimeInForce) -> Self {
        self.time_in_force = Some(tif);
        self
    }

    /// Reduce-only 설정
    pub fn with_reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = Some(reduce_only);
        self
    }

    /// Post-only 설정
    pub fn with_post_only(mut self, post_only: bool) -> Self {
        self.post_only = Some(post_only);
        self
    }
}

impl Default for OrderRequest {
    fn default() -> Self {
        Self::new("", OrderType::Limit, OrderSide::Buy, Decimal::ZERO)
    }
}

/// 트리거 타입 - 스탑 주문의 트리거 가격 기준
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum TriggerType {
    /// 최종 체결가 기준
    #[default]
    Last,
    /// Mark Price 기준
    Mark,
    /// Index Price 기준
    Index,
}


/// 스탑 주문 파라미터
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopOrderParams {
    /// 트리거 가격
    pub trigger_price: Decimal,
    /// 트리거 타입
    #[serde(default)]
    pub trigger_type: TriggerType,
    /// 포지션 감소만
    #[serde(default)]
    pub reduce_only: bool,
    /// 트리거 후 실행할 가격 (limit 주문시)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<Decimal>,
}

impl StopOrderParams {
    /// 새 스탑 주문 파라미터 생성
    pub fn new(trigger_price: Decimal) -> Self {
        Self {
            trigger_price,
            trigger_type: TriggerType::default(),
            reduce_only: false,
            price: None,
        }
    }

    /// 트리거 타입 설정
    pub fn with_trigger_type(mut self, trigger_type: TriggerType) -> Self {
        self.trigger_type = trigger_type;
        self
    }

    /// Reduce-only 설정
    pub fn with_reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = reduce_only;
        self
    }

    /// 실행 가격 설정 (limit)
    pub fn with_price(mut self, price: Decimal) -> Self {
        self.price = Some(price);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_limit_order() {
        let order = Order::limit(
            "12345".into(),
            "BTC/KRW".into(),
            OrderSide::Buy,
            dec!(0.1),
            dec!(50000000),
        );

        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.price, Some(dec!(50000000)));
        assert!(order.is_open());
    }

    #[test]
    fn test_market_order() {
        let order = Order::market("12345".into(), "BTC/KRW".into(), OrderSide::Sell, dec!(0.5));

        assert_eq!(order.order_type, OrderType::Market);
        assert!(order.price.is_none());
    }

    #[test]
    fn test_fill_percentage() {
        let mut order = Order::limit(
            "12345".into(),
            "BTC/KRW".into(),
            OrderSide::Buy,
            dec!(1.0),
            dec!(50000000),
        );
        order.filled = dec!(0.5);

        assert_eq!(order.fill_percentage(), dec!(50));
        assert_eq!(order.calculate_remaining(), dec!(0.5));
    }

    // === OrderRequest tests ===

    #[test]
    fn test_order_request_limit() {
        let req = OrderRequest::limit("BTC/USDT", OrderSide::Buy, dec!(0.1), dec!(50000));

        assert_eq!(req.symbol, "BTC/USDT");
        assert_eq!(req.order_type, OrderType::Limit);
        assert_eq!(req.side, OrderSide::Buy);
        assert_eq!(req.amount, dec!(0.1));
        assert_eq!(req.price, Some(dec!(50000)));
    }

    #[test]
    fn test_order_request_market() {
        let req = OrderRequest::market("ETH/USDT", OrderSide::Sell, dec!(1.0));

        assert_eq!(req.symbol, "ETH/USDT");
        assert_eq!(req.order_type, OrderType::Market);
        assert_eq!(req.side, OrderSide::Sell);
        assert_eq!(req.amount, dec!(1.0));
        assert!(req.price.is_none());
    }

    #[test]
    fn test_order_request_stop_limit() {
        let req = OrderRequest::stop_limit(
            "BTC/USDT",
            OrderSide::Sell,
            dec!(0.5),
            dec!(48000),
            dec!(49000),
        );

        assert_eq!(req.order_type, OrderType::StopLimit);
        assert_eq!(req.price, Some(dec!(48000)));
        assert_eq!(req.stop_price, Some(dec!(49000)));
    }

    #[test]
    fn test_order_request_stop_market() {
        let req = OrderRequest::stop_market("BTC/USDT", OrderSide::Sell, dec!(0.5), dec!(49000));

        assert_eq!(req.order_type, OrderType::StopMarket);
        assert!(req.price.is_none());
        assert_eq!(req.stop_price, Some(dec!(49000)));
    }

    #[test]
    fn test_order_request_builder_pattern() {
        let req = OrderRequest::limit("BTC/USDT", OrderSide::Buy, dec!(0.1), dec!(50000))
            .with_stop_loss(dec!(48000))
            .with_take_profit(dec!(55000))
            .with_client_order_id("my-order-1")
            .with_time_in_force(TimeInForce::GTC)
            .with_reduce_only(true)
            .with_post_only(true);

        assert_eq!(req.stop_loss_price, Some(dec!(48000)));
        assert_eq!(req.take_profit_price, Some(dec!(55000)));
        assert_eq!(req.client_order_id, Some("my-order-1".to_string()));
        assert_eq!(req.time_in_force, Some(TimeInForce::GTC));
        assert_eq!(req.reduce_only, Some(true));
        assert_eq!(req.post_only, Some(true));
    }

    #[test]
    fn test_order_request_serialization() {
        let req = OrderRequest::limit("BTC/USDT", OrderSide::Buy, dec!(0.1), dec!(50000));
        let json = serde_json::to_string(&req).unwrap();

        assert!(json.contains("\"symbol\":\"BTC/USDT\""));
        assert!(json.contains("\"type\":\"limit\""));
        assert!(json.contains("\"side\":\"buy\""));
    }

    // === TriggerType tests ===

    #[test]
    fn test_trigger_type_default() {
        assert_eq!(TriggerType::default(), TriggerType::Last);
    }

    #[test]
    fn test_trigger_type_serialization() {
        let last = TriggerType::Last;
        let mark = TriggerType::Mark;
        let index = TriggerType::Index;

        assert_eq!(serde_json::to_string(&last).unwrap(), "\"last\"");
        assert_eq!(serde_json::to_string(&mark).unwrap(), "\"mark\"");
        assert_eq!(serde_json::to_string(&index).unwrap(), "\"index\"");
    }

    // === StopOrderParams tests ===

    #[test]
    fn test_stop_order_params_new() {
        let params = StopOrderParams::new(dec!(50000));

        assert_eq!(params.trigger_price, dec!(50000));
        assert_eq!(params.trigger_type, TriggerType::Last);
        assert!(!params.reduce_only);
        assert!(params.price.is_none());
    }

    #[test]
    fn test_stop_order_params_builder() {
        let params = StopOrderParams::new(dec!(50000))
            .with_trigger_type(TriggerType::Mark)
            .with_reduce_only(true)
            .with_price(dec!(49500));

        assert_eq!(params.trigger_type, TriggerType::Mark);
        assert!(params.reduce_only);
        assert_eq!(params.price, Some(dec!(49500)));
    }

    #[test]
    fn test_stop_order_params_serialization() {
        let params = StopOrderParams::new(dec!(50000))
            .with_trigger_type(TriggerType::Mark);

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"triggerPrice\":\"50000\""));
        assert!(json.contains("\"triggerType\":\"mark\""));
    }
}
