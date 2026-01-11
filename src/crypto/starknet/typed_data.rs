//! StarkNet Typed Data Signing (SNIP-12)
//!
//! StarkNet의 구조화된 데이터 서명을 제공합니다.
//! EIP-712와 유사하지만 Poseidon 해시를 사용합니다.
//!
//! # 참조
//!
//! - [SNIP-12: Off-chain signing](https://github.com/starknet-io/SNIPs/blob/main/SNIPS/snip-12.md)

use super::poseidon::{poseidon_hash_many, string_to_felt, u64_to_felt};
use crate::errors::{CcxtError, CcxtResult};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;
use std::collections::HashMap;

/// StarkNet 타입 데이터 도메인
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarkNetDomain {
    /// 도메인 이름
    pub name: String,
    /// 도메인 버전
    pub version: String,
    /// 체인 ID (StarkNet chain ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// 리비전 (SNIP-12 revision)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl StarkNetDomain {
    /// 새 도메인 생성
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            chain_id: None,
            revision: None,
        }
    }

    /// 체인 ID 설정
    pub fn with_chain_id(mut self, chain_id: impl Into<String>) -> Self {
        self.chain_id = Some(chain_id.into());
        self
    }

    /// 리비전 설정
    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(revision.into());
        self
    }

    /// 도메인 해시 계산
    pub fn hash(&self) -> Felt {
        let mut values = vec![
            string_to_felt("StarkNetDomain"),
            string_to_felt(&self.name),
            string_to_felt(&self.version),
        ];

        if let Some(ref chain_id) = self.chain_id {
            values.push(string_to_felt(chain_id));
        }

        if let Some(ref revision) = self.revision {
            values.push(string_to_felt(revision));
        }

        poseidon_hash_many(&values)
    }
}

/// StarkNet 타입 데이터 필드
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarkNetTypedDataField {
    /// 필드 이름
    pub name: String,
    /// 필드 타입
    #[serde(rename = "type")]
    pub field_type: String,
}

impl StarkNetTypedDataField {
    pub fn new(name: impl Into<String>, field_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: field_type.into(),
        }
    }
}

/// StarkNet 타입 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StarkNetTypedData {
    /// 타입 정의
    pub types: HashMap<String, Vec<StarkNetTypedDataField>>,
    /// 주 타입 이름
    pub primary_type: String,
    /// 도메인
    pub domain: StarkNetDomain,
    /// 메시지 데이터
    pub message: serde_json::Value,
}

impl StarkNetTypedData {
    /// 새 타입 데이터 생성
    pub fn new(
        domain: StarkNetDomain,
        primary_type: impl Into<String>,
        types: HashMap<String, Vec<StarkNetTypedDataField>>,
        message: serde_json::Value,
    ) -> Self {
        Self {
            types,
            primary_type: primary_type.into(),
            domain,
            message,
        }
    }

    /// 타입 해시 계산
    fn type_hash(&self, type_name: &str) -> CcxtResult<Felt> {
        let encoded = self.encode_type(type_name)?;
        Ok(string_to_felt(&encoded))
    }

    /// 타입 인코딩 문자열 생성
    fn encode_type(&self, type_name: &str) -> CcxtResult<String> {
        let fields = self
            .types
            .get(type_name)
            .ok_or_else(|| CcxtError::InvalidSignature {
                message: format!("Type not found: {type_name}"),
            })?;

        let field_strings: Vec<String> = fields
            .iter()
            .map(|f| format!("{}:{}", f.name, f.field_type))
            .collect();

        Ok(format!("{}({})", type_name, field_strings.join(",")))
    }

    /// 구조체 해시 계산
    fn hash_struct(&self, type_name: &str, data: &serde_json::Value) -> CcxtResult<Felt> {
        let type_hash = self.type_hash(type_name)?;
        let mut values = vec![type_hash];

        let fields = self
            .types
            .get(type_name)
            .ok_or_else(|| CcxtError::InvalidSignature {
                message: format!("Type not found: {type_name}"),
            })?;

        for field in fields {
            let value = data
                .get(&field.name)
                .ok_or_else(|| CcxtError::InvalidSignature {
                    message: format!("Field not found: {}", field.name),
                })?;

            let encoded = self.encode_value(&field.field_type, value)?;
            values.push(encoded);
        }

        Ok(poseidon_hash_many(&values))
    }

    /// 16진수 문자열을 Felt로 파싱 (홀수 자리 처리 포함)
    fn parse_hex_to_felt(hex_str: &str) -> CcxtResult<Felt> {
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);

        // 홀수 길이면 앞에 0을 추가
        let hex_str = if hex_str.len() % 2 != 0 {
            format!("0{hex_str}")
        } else {
            hex_str.to_string()
        };

        let bytes = hex::decode(&hex_str).map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid hex: {e}"),
        })?;

        let mut padded = [0u8; 32];
        let start = 32 - bytes.len().min(32);
        padded[start..].copy_from_slice(&bytes[..bytes.len().min(32)]);
        Ok(Felt::from_bytes_be(&padded))
    }

    /// 값 인코딩
    fn encode_value(&self, field_type: &str, value: &serde_json::Value) -> CcxtResult<Felt> {
        match field_type {
            "felt" | "felt252" => {
                match value {
                    serde_json::Value::String(s) => {
                        // 빈 문자열은 0으로 처리
                        if s.is_empty() {
                            Ok(Felt::ZERO)
                        } else if s.starts_with("0x") || s.starts_with("0X") {
                            // 16진수
                            Self::parse_hex_to_felt(s)
                        } else if s.chars().all(|c| c.is_ascii_digit()) {
                            // 순수 숫자 문자열 - felt로 파싱
                            let n: u64 = s.parse().map_err(|e| CcxtError::InvalidSignature {
                                message: format!("Invalid number: {e}"),
                            })?;
                            Ok(u64_to_felt(n))
                        } else {
                            // 일반 문자열 - short string으로 인코딩
                            Ok(string_to_felt(s))
                        }
                    },
                    serde_json::Value::Number(n) => {
                        if let Some(u) = n.as_u64() {
                            Ok(u64_to_felt(u))
                        } else {
                            Err(CcxtError::InvalidSignature {
                                message: format!("Number too large: {n}"),
                            })
                        }
                    },
                    _ => Err(CcxtError::InvalidSignature {
                        message: format!("Expected felt, got {value:?}"),
                    }),
                }
            },
            "string" | "shortstring" => {
                let s = value.as_str().ok_or_else(|| CcxtError::InvalidSignature {
                    message: format!("Expected string, got {value:?}"),
                })?;
                Ok(string_to_felt(s))
            },
            "u64" | "u128" | "u256" => {
                match value {
                    serde_json::Value::Number(n) => {
                        if let Some(u) = n.as_u64() {
                            Ok(u64_to_felt(u))
                        } else {
                            Err(CcxtError::InvalidSignature {
                                message: format!("Number too large: {n}"),
                            })
                        }
                    },
                    serde_json::Value::String(s) => {
                        // 16진수 또는 10진수 문자열
                        if s.starts_with("0x") {
                            Self::parse_hex_to_felt(s)
                        } else {
                            let n: u64 = s.parse().map_err(|e| CcxtError::InvalidSignature {
                                message: format!("Invalid number: {e}"),
                            })?;
                            Ok(u64_to_felt(n))
                        }
                    },
                    _ => Err(CcxtError::InvalidSignature {
                        message: format!("Expected number, got {value:?}"),
                    }),
                }
            },
            "bool" => {
                let b = value.as_bool().ok_or_else(|| CcxtError::InvalidSignature {
                    message: format!("Expected bool, got {value:?}"),
                })?;
                Ok(if b { Felt::ONE } else { Felt::ZERO })
            },
            // 커스텀 구조체 타입
            _ => {
                if self.types.contains_key(field_type) {
                    self.hash_struct(field_type, value)
                } else {
                    Err(CcxtError::InvalidSignature {
                        message: format!("Unsupported type: {field_type}"),
                    })
                }
            },
        }
    }

    /// 메시지 해시 계산
    pub fn message_hash(&self) -> CcxtResult<Felt> {
        self.hash_struct(&self.primary_type, &self.message)
    }

    /// 전체 서명 해시 계산 (SNIP-12)
    ///
    /// hash = poseidon("StarkNet Message", domain_hash, account_address, message_hash)
    pub fn sign_hash(&self, account_address: &Felt) -> CcxtResult<Felt> {
        let prefix = string_to_felt("StarkNet Message");
        let domain_hash = self.domain.hash();
        let message_hash = self.message_hash()?;

        Ok(poseidon_hash_many(&[
            prefix,
            domain_hash,
            *account_address,
            message_hash,
        ]))
    }
}

/// 타입 데이터 해시 인코딩 (단축 함수)
pub fn encode_typed_data_hash(
    typed_data: &StarkNetTypedData,
    account_address: &Felt,
) -> CcxtResult<Felt> {
    typed_data.sign_hash(account_address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_hash() {
        let domain = StarkNetDomain::new("TestApp", "1").with_chain_id("SN_MAIN");

        let hash = domain.hash();
        assert_ne!(hash, Felt::ZERO);

        // 동일한 도메인은 동일한 해시를 생성
        let hash2 = domain.hash();
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_typed_data_simple() {
        let domain = StarkNetDomain::new("Test", "1");

        let mut types = HashMap::new();
        types.insert(
            "Message".to_string(),
            vec![
                StarkNetTypedDataField::new("content", "string"),
                StarkNetTypedDataField::new("value", "felt"),
            ],
        );

        let message = serde_json::json!({
            "content": "Hello",
            "value": "0x123"
        });

        let typed_data = StarkNetTypedData::new(domain, "Message", types, message);
        let account = Felt::from(12345u64);

        let hash = typed_data.sign_hash(&account).unwrap();
        assert_ne!(hash, Felt::ZERO);
    }

    #[test]
    fn test_encode_type() {
        let domain = StarkNetDomain::new("Test", "1");

        let mut types = HashMap::new();
        types.insert(
            "Order".to_string(),
            vec![
                StarkNetTypedDataField::new("maker", "felt"),
                StarkNetTypedDataField::new("amount", "u128"),
            ],
        );

        let message = serde_json::json!({
            "maker": "0x123",
            "amount": 100
        });

        let typed_data = StarkNetTypedData::new(domain, "Order", types, message);
        let encoded = typed_data.encode_type("Order").unwrap();

        assert!(encoded.contains("Order"));
        assert!(encoded.contains("maker:felt"));
        assert!(encoded.contains("amount:u128"));
    }

    #[test]
    fn test_message_hash() {
        let domain = StarkNetDomain::new("Test", "1");

        let mut types = HashMap::new();
        types.insert(
            "Transfer".to_string(),
            vec![
                StarkNetTypedDataField::new("to", "felt"),
                StarkNetTypedDataField::new("amount", "u64"),
            ],
        );

        let message = serde_json::json!({
            "to": "0x456",
            "amount": 1000
        });

        let typed_data = StarkNetTypedData::new(domain, "Transfer", types, message);
        let hash = typed_data.message_hash().unwrap();

        assert_ne!(hash, Felt::ZERO);
    }
}
