//! Paradex-specific StarkNet Message Signing
//!
//! Paradex 거래소의 SNIP-12 타입 데이터 서명을 제공합니다.
//!
//! # 참조
//!
//! - [Paradex API Authentication](https://docs.paradex.trade/trading/api-authentication)

use crate::errors::CcxtResult;
use super::typed_data::{StarkNetDomain, StarkNetTypedData, StarkNetTypedDataField};
use super::wallet::StarkNetWallet;
use super::poseidon::poseidon_hash_many;
use starknet_types_core::felt::Felt;
use std::collections::HashMap;

/// Paradex 체인 ID
pub const PARADEX_CHAIN_ID_MAINNET: &str = "PRIVATE_SN_POTC_SEPOLIA";
pub const PARADEX_CHAIN_ID_TESTNET: &str = "PRIVATE_SN_POTC_SEPOLIA";

/// Paradex 인증 메시지 빌더
pub struct ParadexAuthMessage {
    /// 체인 ID
    pub chain_id: String,
    /// 타임스탬프 (초)
    pub timestamp: u64,
    /// 만료 시간 (초)
    pub expiry: u64,
}

impl ParadexAuthMessage {
    /// 새 인증 메시지 생성
    pub fn new(chain_id: impl Into<String>, timestamp: u64, expiry: u64) -> Self {
        Self {
            chain_id: chain_id.into(),
            timestamp,
            expiry,
        }
    }

    /// 인증 메시지를 타입 데이터로 변환
    pub fn to_typed_data(&self) -> StarkNetTypedData {
        let domain = StarkNetDomain::new("Paradex", "1")
            .with_chain_id(&self.chain_id);

        let mut types = HashMap::new();
        types.insert(
            "Request".to_string(),
            vec![
                StarkNetTypedDataField::new("method", "felt"),
                StarkNetTypedDataField::new("path", "felt"),
                StarkNetTypedDataField::new("body", "felt"),
                StarkNetTypedDataField::new("timestamp", "felt"),
                StarkNetTypedDataField::new("expiration", "felt"),
            ],
        );

        // POST 메서드를 felt로 인코딩
        let message = serde_json::json!({
            "method": "POST",
            "path": "/v1/auth",
            "body": "",
            "timestamp": self.timestamp.to_string(),
            "expiration": self.expiry.to_string(),
        });

        StarkNetTypedData::new(domain, "Request", types, message)
    }

    /// 지갑으로 서명
    pub fn sign(&self, wallet: &StarkNetWallet) -> CcxtResult<(String, String)> {
        let typed_data = self.to_typed_data();
        let signature = wallet.sign_typed_data(&typed_data)?;
        Ok(signature.to_hex())
    }
}

/// Paradex 온보딩 메시지 빌더
pub struct ParadexOnboardingMessage {
    /// 체인 ID
    pub chain_id: String,
}

impl ParadexOnboardingMessage {
    /// 새 온보딩 메시지 생성
    pub fn new(chain_id: impl Into<String>) -> Self {
        Self {
            chain_id: chain_id.into(),
        }
    }

    /// 온보딩 메시지를 타입 데이터로 변환
    pub fn to_typed_data(&self) -> StarkNetTypedData {
        let domain = StarkNetDomain::new("Paradex", "1")
            .with_chain_id(&self.chain_id);

        let mut types = HashMap::new();
        types.insert(
            "Constant".to_string(),
            vec![
                StarkNetTypedDataField::new("action", "felt"),
            ],
        );

        let message = serde_json::json!({
            "action": "Onboarding",
        });

        StarkNetTypedData::new(domain, "Constant", types, message)
    }

    /// 지갑으로 서명
    pub fn sign(&self, wallet: &StarkNetWallet) -> CcxtResult<(String, String)> {
        let typed_data = self.to_typed_data();
        let signature = wallet.sign_typed_data(&typed_data)?;
        Ok(signature.to_hex())
    }
}

/// Paradex 주문 메시지 빌더
#[derive(Debug, Clone)]
pub struct ParadexOrderMessage {
    /// 체인 ID
    pub chain_id: String,
    /// 타임스탬프 (밀리초)
    pub timestamp: u64,
    /// 마켓 심볼 (예: "ETH-USD-PERP")
    pub market: String,
    /// 주문 방향 (1: BUY, 2: SELL)
    pub side: u8,
    /// 주문 타입 ("LIMIT" 또는 "MARKET")
    pub order_type: String,
    /// 수량 (8자리 소수점)
    pub size: String,
    /// 가격 (8자리 소수점, Limit 주문시)
    pub price: String,
}

impl ParadexOrderMessage {
    /// 새 주문 메시지 생성
    pub fn new(
        chain_id: impl Into<String>,
        market: impl Into<String>,
        side: &str,
        order_type: impl Into<String>,
        size: impl Into<String>,
        price: impl Into<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            chain_id: chain_id.into(),
            timestamp: now,
            market: market.into(),
            side: if side.to_uppercase() == "BUY" { 1 } else { 2 },
            order_type: order_type.into(),
            size: size.into(),
            price: price.into(),
        }
    }

    /// 수량을 체인 형식으로 변환 (8자리 소수점)
    fn chain_size(&self) -> String {
        // 소수점 8자리로 변환 (예: "1.5" -> "150000000")
        parse_decimal_to_quantum(&self.size, 8)
    }

    /// 가격을 체인 형식으로 변환 (8자리 소수점)
    fn chain_price(&self) -> String {
        if self.price.is_empty() || self.price == "0" {
            "0".to_string()
        } else {
            parse_decimal_to_quantum(&self.price, 8)
        }
    }

    /// 주문 메시지를 타입 데이터로 변환
    pub fn to_typed_data(&self) -> StarkNetTypedData {
        let domain = StarkNetDomain::new("Paradex", "1")
            .with_chain_id(&self.chain_id);

        let mut types = HashMap::new();
        types.insert(
            "Order".to_string(),
            vec![
                StarkNetTypedDataField::new("timestamp", "felt"),
                StarkNetTypedDataField::new("market", "felt"),
                StarkNetTypedDataField::new("side", "felt"),
                StarkNetTypedDataField::new("orderType", "felt"),
                StarkNetTypedDataField::new("size", "felt"),
                StarkNetTypedDataField::new("price", "felt"),
            ],
        );

        let message = serde_json::json!({
            "timestamp": self.timestamp.to_string(),
            "market": self.market,
            "side": self.side.to_string(),
            "orderType": self.order_type,
            "size": self.chain_size(),
            "price": self.chain_price(),
        });

        StarkNetTypedData::new(domain, "Order", types, message)
    }

    /// 지갑으로 서명
    pub fn sign(&self, wallet: &StarkNetWallet) -> CcxtResult<(String, String)> {
        let typed_data = self.to_typed_data();
        let signature = wallet.sign_typed_data(&typed_data)?;
        Ok(signature.to_hex())
    }
}

/// Paradex 풀노드 요청 메시지 (POST 요청용)
pub struct ParadexFullNodeMessage {
    /// 체인 ID
    pub chain_id: String,
    /// 계정 주소
    pub account: String,
    /// 요청 페이로드 (JSON)
    pub payload: serde_json::Value,
    /// 타임스탬프 (초)
    pub timestamp: u64,
    /// 서명 버전
    pub version: u8,
}

impl ParadexFullNodeMessage {
    /// 새 풀노드 메시지 생성
    pub fn new(
        chain_id: impl Into<String>,
        account: impl Into<String>,
        payload: serde_json::Value,
        timestamp: u64,
    ) -> Self {
        Self {
            chain_id: chain_id.into(),
            account: account.into(),
            payload,
            timestamp,
            version: 1,
        }
    }

    /// 페이로드 해시 계산 (Poseidon)
    fn payload_hash(&self) -> Felt {
        let payload_str = serde_json::to_string(&self.payload).unwrap_or_default();
        let bytes = payload_str.as_bytes();

        // 페이로드를 Felt 배열로 변환하여 해시
        let mut felts = Vec::new();
        for chunk in bytes.chunks(31) {
            let mut padded = [0u8; 32];
            padded[32 - chunk.len()..].copy_from_slice(chunk);
            felts.push(Felt::from_bytes_be(&padded));
        }

        if felts.is_empty() {
            Felt::ZERO
        } else {
            poseidon_hash_many(&felts)
        }
    }

    /// 풀노드 메시지를 타입 데이터로 변환
    pub fn to_typed_data(&self) -> StarkNetTypedData {
        let domain = StarkNetDomain::new("Paradex", "1")
            .with_chain_id(&self.chain_id);

        let mut types = HashMap::new();
        types.insert(
            "Request".to_string(),
            vec![
                StarkNetTypedDataField::new("account", "felt"),
                StarkNetTypedDataField::new("payload", "felt"),
                StarkNetTypedDataField::new("timestamp", "felt"),
                StarkNetTypedDataField::new("version", "felt"),
            ],
        );

        let payload_hash = self.payload_hash();
        let payload_hex = format!("0x{}", hex::encode(payload_hash.to_bytes_be()));

        let message = serde_json::json!({
            "account": self.account,
            "payload": payload_hex,
            "timestamp": self.timestamp.to_string(),
            "version": self.version.to_string(),
        });

        StarkNetTypedData::new(domain, "Request", types, message)
    }

    /// 지갑으로 서명
    pub fn sign(&self, wallet: &StarkNetWallet) -> CcxtResult<(String, String)> {
        let typed_data = self.to_typed_data();
        let signature = wallet.sign_typed_data(&typed_data)?;
        Ok(signature.to_hex())
    }
}

/// 소수점 문자열을 quantum 값으로 변환
///
/// 예: "1.5"를 8자리 소수점으로 -> "150000000"
fn parse_decimal_to_quantum(value: &str, decimals: u32) -> String {
    let parts: Vec<&str> = value.split('.').collect();
    let integer_part = parts.first().unwrap_or(&"0");
    let fractional_part = parts.get(1).unwrap_or(&"");

    // 소수점 이하 부분을 decimals 자리로 패딩
    let mut frac = fractional_part.to_string();
    while frac.len() < decimals as usize {
        frac.push('0');
    }
    // 필요시 자르기
    frac.truncate(decimals as usize);

    // 정수부와 소수부 결합
    let result = format!("{integer_part}{frac}");

    // 앞의 0 제거
    result.trim_start_matches('0').to_string()
        .chars()
        .next()
        .map_or("0".to_string(), |_| result.trim_start_matches('0').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_message() {
        let msg = ParadexAuthMessage::new("SN_MAIN", 1234567890, 1234568070);
        let typed_data = msg.to_typed_data();

        assert_eq!(typed_data.primary_type, "Request");
        assert!(typed_data.types.contains_key("Request"));
    }

    #[test]
    fn test_onboarding_message() {
        let msg = ParadexOnboardingMessage::new("SN_MAIN");
        let typed_data = msg.to_typed_data();

        assert_eq!(typed_data.primary_type, "Constant");
    }

    #[test]
    fn test_order_message() {
        let msg = ParadexOrderMessage::new(
            "SN_MAIN",
            "ETH-USD-PERP",
            "BUY",
            "LIMIT",
            "1.5",
            "2000.0",
        );

        assert_eq!(msg.side, 1);
        assert_eq!(msg.chain_size(), "150000000");
        assert_eq!(msg.chain_price(), "200000000000");
    }

    #[test]
    fn test_parse_decimal_to_quantum() {
        assert_eq!(parse_decimal_to_quantum("1.5", 8), "150000000");
        assert_eq!(parse_decimal_to_quantum("100", 8), "10000000000");
        assert_eq!(parse_decimal_to_quantum("0.00001", 8), "1000");
        assert_eq!(parse_decimal_to_quantum("2000.12345678", 8), "200012345678");
    }

    #[test]
    fn test_sign_auth_message() {
        let wallet = StarkNetWallet::new(Felt::from(12345u64), "paradex");
        let msg = ParadexAuthMessage::new("SN_MAIN", 1234567890, 1234568070);

        let (r, s) = msg.sign(&wallet).unwrap();
        assert!(r.starts_with("0x"));
        assert!(s.starts_with("0x"));
    }
}
