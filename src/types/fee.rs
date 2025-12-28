//! Fee type - 수수료 정보

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 수수료 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct Fee {
    /// 수수료 금액
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Decimal>,
    /// 수수료 화폐
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// 수수료율 (%)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<Decimal>,
}

impl Fee {
    /// 새 Fee 생성
    pub fn new(cost: Decimal, currency: String) -> Self {
        Self {
            cost: Some(cost),
            currency: Some(currency),
            rate: None,
        }
    }

    /// 수수료율로 생성
    pub fn from_rate(rate: Decimal, currency: String) -> Self {
        Self {
            cost: None,
            currency: Some(currency),
            rate: Some(rate),
        }
    }

    /// 수수료율 설정
    pub fn with_rate(mut self, rate: Decimal) -> Self {
        self.rate = Some(rate);
        self
    }

    /// 비용 계산 (주어진 금액에 대해)
    pub fn calculate_cost(&self, amount: Decimal) -> Option<Decimal> {
        self.rate.map(|r| amount * r)
    }
}


/// 거래 수수료 구조 (기존 - 하위 호환성 유지)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TradingFees {
    /// 메이커 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maker: Option<Decimal>,
    /// 테이커 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taker: Option<Decimal>,
    /// 퍼센트 기반
    #[serde(default)]
    pub percentage: bool,
    /// 티어 기반
    #[serde(default)]
    pub tier_based: bool,
}

impl TradingFees {
    /// 새 TradingFees 생성
    pub fn new(maker: Decimal, taker: Decimal) -> Self {
        Self {
            maker: Some(maker),
            taker: Some(taker),
            percentage: true,
            tier_based: false,
        }
    }
}

/// 거래 수수료 (심볼별) - CCXT 표준 형식
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradingFee {
    /// 심볼 (예: "BTC/USDT")
    pub symbol: String,
    /// 메이커 수수료율
    pub maker: Decimal,
    /// 테이커 수수료율
    pub taker: Decimal,
    /// 퍼센트 기반 여부
    #[serde(default = "default_true")]
    pub percentage: bool,
    /// 티어 기반 여부
    #[serde(default)]
    pub tier_based: bool,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

fn default_true() -> bool {
    true
}

impl TradingFee {
    /// 새 TradingFee 생성
    pub fn new(symbol: &str, maker: Decimal, taker: Decimal) -> Self {
        Self {
            symbol: symbol.to_string(),
            maker,
            taker,
            percentage: true,
            tier_based: false,
            info: serde_json::Value::Null,
        }
    }

    /// 원본 응답 설정
    pub fn with_info(mut self, info: serde_json::Value) -> Self {
        self.info = info;
        self
    }

    /// 티어 기반 여부 설정
    pub fn with_tier_based(mut self, tier_based: bool) -> Self {
        self.tier_based = tier_based;
        self
    }

    /// 수수료 계산 (거래금액 기준)
    pub fn calculate_maker_fee(&self, amount: Decimal) -> Decimal {
        if self.percentage {
            amount * self.maker
        } else {
            self.maker
        }
    }

    /// 테이커 수수료 계산
    pub fn calculate_taker_fee(&self, amount: Decimal) -> Decimal {
        if self.percentage {
            amount * self.taker
        } else {
            self.taker
        }
    }
}

impl Default for TradingFee {
    fn default() -> Self {
        Self {
            symbol: String::new(),
            maker: Decimal::ZERO,
            taker: Decimal::ZERO,
            percentage: true,
            tier_based: false,
            info: serde_json::Value::Null,
        }
    }
}

/// 수수료 정보 (입출금용)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeInfo {
    /// 수수료 금액
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Decimal>,
    /// 퍼센트 기반 여부
    #[serde(default)]
    pub percentage: bool,
}

impl FeeInfo {
    /// 고정 수수료 생성
    pub fn fixed(fee: Decimal) -> Self {
        Self {
            fee: Some(fee),
            percentage: false,
        }
    }

    /// 퍼센트 수수료 생성
    pub fn percent(rate: Decimal) -> Self {
        Self {
            fee: Some(rate),
            percentage: true,
        }
    }

    /// 수수료 계산
    pub fn calculate(&self, amount: Decimal) -> Option<Decimal> {
        self.fee.map(|f| {
            if self.percentage {
                amount * f
            } else {
                f
            }
        })
    }
}

/// 네트워크별 수수료
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkFee {
    /// 네트워크 이름 (예: "ETH", "TRC20", "BEP20")
    pub network: String,
    /// 입금 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit: Option<FeeInfo>,
    /// 출금 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdraw: Option<FeeInfo>,
    /// 입금 가능 여부
    #[serde(default = "default_true")]
    pub deposit_enabled: bool,
    /// 출금 가능 여부
    #[serde(default = "default_true")]
    pub withdraw_enabled: bool,
}

impl NetworkFee {
    /// 새 NetworkFee 생성
    pub fn new(network: &str) -> Self {
        Self {
            network: network.to_string(),
            deposit: None,
            withdraw: None,
            deposit_enabled: true,
            withdraw_enabled: true,
        }
    }

    /// 입금 수수료 설정
    pub fn with_deposit(mut self, fee: FeeInfo) -> Self {
        self.deposit = Some(fee);
        self
    }

    /// 출금 수수료 설정
    pub fn with_withdraw(mut self, fee: FeeInfo) -> Self {
        self.withdraw = Some(fee);
        self
    }

    /// 입금 활성화 여부 설정
    pub fn with_deposit_enabled(mut self, enabled: bool) -> Self {
        self.deposit_enabled = enabled;
        self
    }

    /// 출금 활성화 여부 설정
    pub fn with_withdraw_enabled(mut self, enabled: bool) -> Self {
        self.withdraw_enabled = enabled;
        self
    }
}

/// 입출금 수수료 (통화별)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositWithdrawFee {
    /// 통화 코드 (예: "BTC", "ETH")
    pub currency: String,
    /// 기본 입금 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit: Option<FeeInfo>,
    /// 기본 출금 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdraw: Option<FeeInfo>,
    /// 네트워크별 수수료
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub networks: HashMap<String, NetworkFee>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

impl DepositWithdrawFee {
    /// 새 DepositWithdrawFee 생성
    pub fn new(currency: &str) -> Self {
        Self {
            currency: currency.to_string(),
            deposit: None,
            withdraw: None,
            networks: HashMap::new(),
            info: serde_json::Value::Null,
        }
    }

    /// 기본 입금 수수료 설정
    pub fn with_deposit(mut self, fee: FeeInfo) -> Self {
        self.deposit = Some(fee);
        self
    }

    /// 기본 출금 수수료 설정
    pub fn with_withdraw(mut self, fee: FeeInfo) -> Self {
        self.withdraw = Some(fee);
        self
    }

    /// 네트워크 추가
    pub fn add_network(mut self, network: NetworkFee) -> Self {
        self.networks.insert(network.network.clone(), network);
        self
    }

    /// 원본 응답 설정
    pub fn with_info(mut self, info: serde_json::Value) -> Self {
        self.info = info;
        self
    }

    /// 특정 네트워크의 출금 수수료 조회
    pub fn get_withdraw_fee(&self, network: Option<&str>) -> Option<&FeeInfo> {
        match network {
            Some(n) => self.networks.get(n).and_then(|nf| nf.withdraw.as_ref()),
            None => self.withdraw.as_ref(),
        }
    }

    /// 특정 네트워크의 입금 수수료 조회
    pub fn get_deposit_fee(&self, network: Option<&str>) -> Option<&FeeInfo> {
        match network {
            Some(n) => self.networks.get(n).and_then(|nf| nf.deposit.as_ref()),
            None => self.deposit.as_ref(),
        }
    }
}

/// 거래소 상태 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeStatus {
    /// 거래소 상태 ("ok", "maintenance", "error" 등)
    pub status: String,
    /// 마지막 업데이트 타임스탬프
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<i64>,
    /// ETA (예상 복구 시간)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta: Option<String>,
    /// 상태 URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

impl ExchangeStatus {
    /// 정상 상태 생성
    pub fn ok() -> Self {
        Self {
            status: "ok".to_string(),
            updated: Some(chrono::Utc::now().timestamp_millis()),
            eta: None,
            url: None,
            info: serde_json::Value::Null,
        }
    }

    /// 점검 상태 생성
    pub fn maintenance(eta: Option<&str>) -> Self {
        Self {
            status: "maintenance".to_string(),
            updated: Some(chrono::Utc::now().timestamp_millis()),
            eta: eta.map(|s| s.to_string()),
            url: None,
            info: serde_json::Value::Null,
        }
    }

    /// 정상 상태인지 확인
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }

    /// 점검 중인지 확인
    pub fn is_maintenance(&self) -> bool {
        self.status == "maintenance"
    }
}

impl Default for ExchangeStatus {
    fn default() -> Self {
        Self::ok()
    }
}

/// 최우선 호가 (Bid/Ask)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BidAsk {
    /// 심볼
    pub symbol: String,
    /// 최우선 매수호가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid: Option<Decimal>,
    /// 최우선 매수수량
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid_volume: Option<Decimal>,
    /// 최우선 매도호가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Decimal>,
    /// 최우선 매도수량
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_volume: Option<Decimal>,
    /// 타임스탬프
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

impl BidAsk {
    /// 새 BidAsk 생성
    pub fn new(symbol: &str) -> Self {
        Self {
            symbol: symbol.to_string(),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            timestamp: None,
            datetime: None,
            info: serde_json::Value::Null,
        }
    }

    /// Bid 설정
    pub fn with_bid(mut self, price: Decimal, volume: Option<Decimal>) -> Self {
        self.bid = Some(price);
        self.bid_volume = volume;
        self
    }

    /// Ask 설정
    pub fn with_ask(mut self, price: Decimal, volume: Option<Decimal>) -> Self {
        self.ask = Some(price);
        self.ask_volume = volume;
        self
    }

    /// 타임스탬프 설정
    pub fn with_timestamp(mut self, ts: i64) -> Self {
        self.timestamp = Some(ts);
        self.datetime = Some(
            chrono::DateTime::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        self
    }

    /// 스프레드 계산
    pub fn spread(&self) -> Option<Decimal> {
        match (self.ask, self.bid) {
            (Some(ask), Some(bid)) if bid > Decimal::ZERO => Some((ask - bid) / bid),
            _ => None,
        }
    }

    /// 중간값 계산
    pub fn mid(&self) -> Option<Decimal> {
        match (self.ask, self.bid) {
            (Some(ask), Some(bid)) => Some((ask + bid) / Decimal::TWO),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_fee() {
        let fee = Fee::new(dec!(100), "KRW".into());
        assert_eq!(fee.cost, Some(dec!(100)));
        assert_eq!(fee.currency, Some("KRW".into()));
    }

    #[test]
    fn test_fee_from_rate() {
        let fee = Fee::from_rate(dec!(0.001), "BTC".into());
        assert_eq!(fee.calculate_cost(dec!(1.0)), Some(dec!(0.001)));
    }

    #[test]
    fn test_trading_fees() {
        let fees = TradingFees::new(dec!(0.0005), dec!(0.001));
        assert_eq!(fees.maker, Some(dec!(0.0005)));
        assert_eq!(fees.taker, Some(dec!(0.001)));
    }

    // === TradingFee tests ===

    #[test]
    fn test_trading_fee_new() {
        let fee = TradingFee::new("BTC/USDT", dec!(0.001), dec!(0.002));

        assert_eq!(fee.symbol, "BTC/USDT");
        assert_eq!(fee.maker, dec!(0.001));
        assert_eq!(fee.taker, dec!(0.002));
        assert!(fee.percentage);
        assert!(!fee.tier_based);
    }

    #[test]
    fn test_trading_fee_calculate() {
        let fee = TradingFee::new("BTC/USDT", dec!(0.001), dec!(0.002));

        assert_eq!(fee.calculate_maker_fee(dec!(1000)), dec!(1));
        assert_eq!(fee.calculate_taker_fee(dec!(1000)), dec!(2));
    }

    #[test]
    fn test_trading_fee_serialization() {
        let fee = TradingFee::new("ETH/USDT", dec!(0.0005), dec!(0.001));
        let json = serde_json::to_string(&fee).unwrap();

        assert!(json.contains("\"symbol\":\"ETH/USDT\""));
        assert!(json.contains("\"maker\":"));
        assert!(json.contains("\"taker\":"));
    }

    // === FeeInfo tests ===

    #[test]
    fn test_fee_info_fixed() {
        let fee = FeeInfo::fixed(dec!(0.0005));

        assert_eq!(fee.fee, Some(dec!(0.0005)));
        assert!(!fee.percentage);
        assert_eq!(fee.calculate(dec!(1000)), Some(dec!(0.0005)));
    }

    #[test]
    fn test_fee_info_percent() {
        let fee = FeeInfo::percent(dec!(0.001));

        assert_eq!(fee.fee, Some(dec!(0.001)));
        assert!(fee.percentage);
        assert_eq!(fee.calculate(dec!(1000)), Some(dec!(1)));
    }

    // === NetworkFee tests ===

    #[test]
    fn test_network_fee() {
        let fee = NetworkFee::new("ETH")
            .with_deposit(FeeInfo::fixed(Decimal::ZERO))
            .with_withdraw(FeeInfo::fixed(dec!(0.001)))
            .with_deposit_enabled(true)
            .with_withdraw_enabled(true);

        assert_eq!(fee.network, "ETH");
        assert!(fee.deposit.is_some());
        assert!(fee.withdraw.is_some());
        assert_eq!(fee.withdraw.unwrap().fee, Some(dec!(0.001)));
    }

    // === DepositWithdrawFee tests ===

    #[test]
    fn test_deposit_withdraw_fee_new() {
        let fee = DepositWithdrawFee::new("USDT");

        assert_eq!(fee.currency, "USDT");
        assert!(fee.deposit.is_none());
        assert!(fee.withdraw.is_none());
        assert!(fee.networks.is_empty());
    }

    #[test]
    fn test_deposit_withdraw_fee_with_networks() {
        let fee = DepositWithdrawFee::new("USDT")
            .with_withdraw(FeeInfo::fixed(dec!(1)))
            .add_network(
                NetworkFee::new("TRC20")
                    .with_withdraw(FeeInfo::fixed(dec!(0.5)))
            )
            .add_network(
                NetworkFee::new("ERC20")
                    .with_withdraw(FeeInfo::fixed(dec!(5)))
            );

        assert_eq!(fee.networks.len(), 2);

        // Default withdraw fee
        let default_fee = fee.get_withdraw_fee(None);
        assert!(default_fee.is_some());
        assert_eq!(default_fee.unwrap().fee, Some(dec!(1)));

        // TRC20 withdraw fee
        let trc20_fee = fee.get_withdraw_fee(Some("TRC20"));
        assert!(trc20_fee.is_some());
        assert_eq!(trc20_fee.unwrap().fee, Some(dec!(0.5)));

        // ERC20 withdraw fee
        let erc20_fee = fee.get_withdraw_fee(Some("ERC20"));
        assert!(erc20_fee.is_some());
        assert_eq!(erc20_fee.unwrap().fee, Some(dec!(5)));
    }

    // === ExchangeStatus tests ===

    #[test]
    fn test_exchange_status_ok() {
        let status = ExchangeStatus::ok();

        assert_eq!(status.status, "ok");
        assert!(status.is_ok());
        assert!(!status.is_maintenance());
        assert!(status.updated.is_some());
    }

    #[test]
    fn test_exchange_status_maintenance() {
        let status = ExchangeStatus::maintenance(Some("2 hours"));

        assert_eq!(status.status, "maintenance");
        assert!(!status.is_ok());
        assert!(status.is_maintenance());
        assert_eq!(status.eta, Some("2 hours".to_string()));
    }

    // === BidAsk tests ===

    #[test]
    fn test_bid_ask_new() {
        let ba = BidAsk::new("BTC/USDT");

        assert_eq!(ba.symbol, "BTC/USDT");
        assert!(ba.bid.is_none());
        assert!(ba.ask.is_none());
    }

    #[test]
    fn test_bid_ask_with_values() {
        let ba = BidAsk::new("BTC/USDT")
            .with_bid(dec!(50000), Some(dec!(1.5)))
            .with_ask(dec!(50100), Some(dec!(2.0)))
            .with_timestamp(1699999999999);

        assert_eq!(ba.bid, Some(dec!(50000)));
        assert_eq!(ba.bid_volume, Some(dec!(1.5)));
        assert_eq!(ba.ask, Some(dec!(50100)));
        assert_eq!(ba.ask_volume, Some(dec!(2.0)));
        assert!(ba.timestamp.is_some());
        assert!(ba.datetime.is_some());
    }

    #[test]
    fn test_bid_ask_spread() {
        let ba = BidAsk::new("BTC/USDT")
            .with_bid(dec!(50000), None)
            .with_ask(dec!(50100), None);

        let spread = ba.spread().unwrap();
        // (50100 - 50000) / 50000 = 0.002
        assert_eq!(spread, dec!(0.002));
    }

    #[test]
    fn test_bid_ask_mid() {
        let ba = BidAsk::new("BTC/USDT")
            .with_bid(dec!(50000), None)
            .with_ask(dec!(50100), None);

        let mid = ba.mid().unwrap();
        // (50000 + 50100) / 2 = 50050
        assert_eq!(mid, dec!(50050));
    }

    #[test]
    fn test_bid_ask_missing_values() {
        let ba = BidAsk::new("BTC/USDT")
            .with_bid(dec!(50000), None);

        assert!(ba.spread().is_none());
        assert!(ba.mid().is_none());
    }
}
