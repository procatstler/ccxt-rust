//! Market type - 거래소 마켓 정보

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 마켓 타입
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MarketType {
    #[default]
    Spot,
    Margin,
    Swap,
    Future,
    Option,
}

/// 마켓 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Market {
    /// 거래소 내부 ID (예: 'KRW-BTC')
    pub id: String,
    /// 소문자 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lowercase_id: Option<String>,
    /// 통합 심볼 (예: 'BTC/KRW')
    pub symbol: String,
    /// 기준 화폐 (예: 'BTC')
    pub base: String,
    /// 견적 화폐 (예: 'KRW')
    pub quote: String,
    /// 정산 화폐 (파생상품용)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settle: Option<String>,
    /// 거래소 기준 화폐 ID
    pub base_id: String,
    /// 거래소 견적 화폐 ID
    pub quote_id: String,
    /// 거래소 정산 화폐 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settle_id: Option<String>,
    /// 마켓 타입
    #[serde(rename = "type")]
    pub market_type: MarketType,
    /// 현물 여부
    pub spot: bool,
    /// 마진 여부
    pub margin: bool,
    /// 스왑 여부
    pub swap: bool,
    /// 선물 여부
    pub future: bool,
    /// 옵션 여부
    pub option: bool,
    /// 인덱스 여부
    #[serde(default)]
    pub index: bool,
    /// 활성 상태
    pub active: bool,
    /// 계약 여부
    pub contract: bool,
    /// 선형 계약 여부
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linear: Option<bool>,
    /// 역방향 계약 여부
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inverse: Option<bool>,
    /// 서브타입
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_type: Option<String>,
    /// 테이커 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taker: Option<Decimal>,
    /// 메이커 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maker: Option<Decimal>,
    /// 계약 크기
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_size: Option<Decimal>,
    /// 만기일 (timestamp)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry: Option<i64>,
    /// 만기일 (datetime string)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry_datetime: Option<String>,
    /// 행사가 (옵션)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strike: Option<Decimal>,
    /// 옵션 타입 (call/put)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option_type: Option<String>,
    /// 기초자산 심볼 (옵션, 예: "BTC")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying: Option<String>,
    /// 기초자산 거래소 ID (옵션)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_id: Option<String>,
    /// 정밀도
    pub precision: MarketPrecision,
    /// 거래 제한
    pub limits: MarketLimits,
    /// 마진 모드
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_modes: Option<MarginModes>,
    /// 생성일
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
    /// 티어 기반 수수료
    #[serde(default)]
    pub tier_based: bool,
    /// 퍼센트 기반 수수료
    #[serde(default)]
    pub percentage: bool,
}

/// 마켓 정밀도
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketPrecision {
    /// 수량 정밀도
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<i32>,
    /// 가격 정밀도
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<i32>,
    /// 비용 정밀도
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<i32>,
    /// 기준 정밀도
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<i32>,
    /// 견적 정밀도
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<i32>,
}

/// 마켓 제한
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketLimits {
    /// 수량 제한
    #[serde(default)]
    pub amount: MinMax,
    /// 가격 제한
    #[serde(default)]
    pub price: MinMax,
    /// 비용 제한
    #[serde(default)]
    pub cost: MinMax,
    /// 레버리지 제한
    #[serde(default)]
    pub leverage: MinMax,
}

/// 최소/최대 값
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MinMax {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<Decimal>,
}

/// 마진 모드
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarginModes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolated: Option<bool>,
}

impl Market {
    /// 현물 마켓 생성
    pub fn spot(id: String, symbol: String, base: String, quote: String) -> Self {
        Self {
            id: id.clone(),
            lowercase_id: Some(id.to_lowercase()),
            symbol,
            base: base.clone(),
            quote: quote.clone(),
            settle: None,
            base_id: base,
            quote_id: quote,
            settle_id: None,
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            index: false,
            active: true,
            contract: false,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: None,
            maker: None,
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            underlying: None,
            underlying_id: None,
            precision: MarketPrecision::default(),
            limits: MarketLimits::default(),
            margin_modes: None,
            created: None,
            info: serde_json::Value::Null,
            tier_based: false,
            percentage: true,
        }
    }

    /// 옵션 마켓 생성
    pub fn option_market(
        id: String,
        symbol: String,
        base: String,
        quote: String,
        underlying: String,
        strike: Decimal,
        option_type: &str,
        expiry: Option<i64>,
    ) -> Self {
        Self {
            id: id.clone(),
            lowercase_id: Some(id.to_lowercase()),
            symbol,
            base: base.clone(),
            quote: quote.clone(),
            settle: Some(quote.clone()),
            base_id: base,
            quote_id: quote.clone(),
            settle_id: Some(quote),
            market_type: MarketType::Option,
            spot: false,
            margin: false,
            swap: false,
            future: false,
            option: true,
            index: false,
            active: true,
            contract: true,
            linear: Some(true),
            inverse: Some(false),
            sub_type: None,
            taker: None,
            maker: None,
            contract_size: Some(Decimal::ONE),
            expiry,
            expiry_datetime: expiry.map(|ts| {
                DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            strike: Some(strike),
            option_type: Some(option_type.to_string()),
            underlying: Some(underlying.clone()),
            underlying_id: Some(underlying),
            precision: MarketPrecision::default(),
            limits: MarketLimits::default(),
            margin_modes: None,
            created: None,
            info: serde_json::Value::Null,
            tier_based: false,
            percentage: true,
        }
    }

    /// 선물/스왑 마켓 생성
    pub fn futures(
        id: String,
        symbol: String,
        base: String,
        quote: String,
        settle: String,
        is_linear: bool,
        expiry: Option<i64>,
    ) -> Self {
        let is_perpetual = expiry.is_none();
        Self {
            id: id.clone(),
            lowercase_id: Some(id.to_lowercase()),
            symbol,
            base: base.clone(),
            quote: quote.clone(),
            settle: Some(settle.clone()),
            base_id: base,
            quote_id: quote,
            settle_id: Some(settle),
            market_type: if is_perpetual { MarketType::Swap } else { MarketType::Future },
            spot: false,
            margin: false,
            swap: is_perpetual,
            future: !is_perpetual,
            option: false,
            index: false,
            active: true,
            contract: true,
            linear: Some(is_linear),
            inverse: Some(!is_linear),
            sub_type: Some(if is_linear { "linear".to_string() } else { "inverse".to_string() }),
            taker: None,
            maker: None,
            contract_size: Some(Decimal::ONE),
            expiry,
            expiry_datetime: expiry.map(|ts| {
                DateTime::<Utc>::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            strike: None,
            option_type: None,
            underlying: None,
            underlying_id: None,
            precision: MarketPrecision::default(),
            limits: MarketLimits::default(),
            margin_modes: None,
            created: None,
            info: serde_json::Value::Null,
            tier_based: false,
            percentage: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_spot_market() {
        let market = Market::spot(
            "KRW-BTC".into(),
            "BTC/KRW".into(),
            "BTC".into(),
            "KRW".into(),
        );
        assert!(market.spot);
        assert!(!market.margin);
        assert_eq!(market.market_type, MarketType::Spot);
    }

    #[test]
    fn test_option_market() {
        let market = Market::option_market(
            "BTC-241227-100000-C".into(),
            "BTC/USDT:USDT-241227-100000-C".into(),
            "BTC".into(),
            "USDT".into(),
            "BTC".into(),
            dec!(100000),
            "call",
            Some(1735257600000), // 2024-12-27
        );
        assert!(market.option);
        assert!(!market.spot);
        assert!(market.contract);
        assert_eq!(market.market_type, MarketType::Option);
        assert_eq!(market.strike, Some(dec!(100000)));
        assert_eq!(market.option_type, Some("call".to_string()));
        assert_eq!(market.underlying, Some("BTC".to_string()));
        assert!(market.expiry_datetime.is_some());
    }

    #[test]
    fn test_futures_perpetual() {
        let market = Market::futures(
            "BTCUSDT".into(),
            "BTC/USDT:USDT".into(),
            "BTC".into(),
            "USDT".into(),
            "USDT".into(),
            true, // linear
            None, // perpetual
        );
        assert!(market.swap);
        assert!(!market.future);
        assert!(market.contract);
        assert_eq!(market.market_type, MarketType::Swap);
        assert_eq!(market.linear, Some(true));
        assert_eq!(market.inverse, Some(false));
        assert_eq!(market.sub_type, Some("linear".to_string()));
    }

    #[test]
    fn test_futures_dated() {
        let market = Market::futures(
            "BTC-241227".into(),
            "BTC/USDT:USDT-241227".into(),
            "BTC".into(),
            "USDT".into(),
            "USDT".into(),
            true,
            Some(1735257600000), // 2024-12-27
        );
        assert!(!market.swap);
        assert!(market.future);
        assert!(market.contract);
        assert_eq!(market.market_type, MarketType::Future);
        assert!(market.expiry.is_some());
        assert!(market.expiry_datetime.is_some());
    }

    #[test]
    fn test_inverse_futures() {
        let market = Market::futures(
            "BTCUSD".into(),
            "BTC/USD:BTC".into(),
            "BTC".into(),
            "USD".into(),
            "BTC".into(),
            false, // inverse
            None,  // perpetual
        );
        assert!(market.swap);
        assert_eq!(market.linear, Some(false));
        assert_eq!(market.inverse, Some(true));
        assert_eq!(market.sub_type, Some("inverse".to_string()));
    }
}
