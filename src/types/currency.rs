//! Currency type - 화폐 정보

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::MinMax;

/// 화폐 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Currency {
    /// 화폐 ID (거래소 내부)
    pub id: String,
    /// 통합 코드 (예: 'BTC')
    pub code: String,
    /// 이름
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 활성 상태
    pub active: bool,
    /// 입금 가능
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit: Option<bool>,
    /// 출금 가능
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdraw: Option<bool>,
    /// 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Decimal>,
    /// 정밀도
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<i32>,
    /// 제한
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<CurrencyLimits>,
    /// 네트워크
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub networks: HashMap<String, Network>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

/// 화폐 제한
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CurrencyLimits {
    /// 출금 제한
    #[serde(default)]
    pub withdraw: MinMax,
    /// 입금 제한
    #[serde(default)]
    pub deposit: MinMax,
}

/// 네트워크 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    /// 네트워크 ID
    pub id: String,
    /// 네트워크 이름
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// 활성 상태
    pub active: bool,
    /// 입금 가능
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit: Option<bool>,
    /// 출금 가능
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdraw: Option<bool>,
    /// 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Decimal>,
    /// 정밀도
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<i32>,
    /// 제한
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<CurrencyLimits>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

impl Currency {
    /// 새 Currency 생성
    pub fn new(id: String, code: String) -> Self {
        Self {
            id,
            code,
            name: None,
            active: true,
            deposit: None,
            withdraw: None,
            fee: None,
            precision: None,
            limits: None,
            networks: HashMap::new(),
            info: serde_json::Value::Null,
        }
    }

    /// 이름 설정
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    /// 수수료 설정
    pub fn with_fee(mut self, fee: Decimal) -> Self {
        self.fee = Some(fee);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_currency() {
        let currency = Currency::new("BTC".into(), "BTC".into())
            .with_name("Bitcoin")
            .with_fee(dec!(0.0005));

        assert_eq!(currency.code, "BTC");
        assert_eq!(currency.name, Some("Bitcoin".into()));
        assert_eq!(currency.fee, Some(dec!(0.0005)));
    }
}
