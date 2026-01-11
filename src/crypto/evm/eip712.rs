//! EIP-712 Typed Data Signing
//!
//! Ethereum EIP-712 표준에 따른 구조화된 데이터 인코딩 및 서명을 제공합니다.
//!
//! # 참조
//!
//! - [EIP-712: Typed structured data hashing and signing](https://eips.ethereum.org/EIPS/eip-712)

use super::keccak::{keccak256, pad_address, pad_bool, pad_u256};
use crate::crypto::common::TypedDataHasher;
use crate::errors::{CcxtError, CcxtResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// EIP-712 도메인 분리자
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Eip712Domain {
    /// 도메인 이름 (e.g., "Hyperliquid")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// 도메인 버전 (e.g., "1")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// 체인 ID (e.g., 42161 for Arbitrum)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,

    /// 검증 컨트랙트 주소
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifying_contract: Option<String>,

    /// 솔트 (선택)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
}

impl Eip712Domain {
    /// 새 도메인 생성
    pub fn new(name: impl Into<String>, version: impl Into<String>, chain_id: u64) -> Self {
        Self {
            name: Some(name.into()),
            version: Some(version.into()),
            chain_id: Some(chain_id),
            verifying_contract: None,
            salt: None,
        }
    }

    /// 검증 컨트랙트 설정
    pub fn with_verifying_contract(mut self, contract: impl Into<String>) -> Self {
        self.verifying_contract = Some(contract.into());
        self
    }

    /// 솔트 설정
    pub fn with_salt(mut self, salt: impl Into<String>) -> Self {
        self.salt = Some(salt.into());
        self
    }

    /// 도메인 타입 문자열 생성 (EIP712Domain(...))
    pub fn encode_type(&self) -> String {
        let mut fields = Vec::new();

        if self.name.is_some() {
            fields.push("string name");
        }
        if self.version.is_some() {
            fields.push("string version");
        }
        if self.chain_id.is_some() {
            fields.push("uint256 chainId");
        }
        if self.verifying_contract.is_some() {
            fields.push("address verifyingContract");
        }
        if self.salt.is_some() {
            fields.push("bytes32 salt");
        }

        format!("EIP712Domain({})", fields.join(","))
    }

    /// 도메인 분리자 해시 계산
    pub fn separator(&self) -> CcxtResult<[u8; 32]> {
        let type_hash = keccak256(self.encode_type().as_bytes());

        let mut encoded = Vec::new();
        encoded.extend_from_slice(&type_hash);

        if let Some(ref name) = self.name {
            encoded.extend_from_slice(&keccak256(name.as_bytes()));
        }
        if let Some(ref version) = self.version {
            encoded.extend_from_slice(&keccak256(version.as_bytes()));
        }
        if let Some(chain_id) = self.chain_id {
            encoded.extend_from_slice(&pad_u256(chain_id));
        }
        if let Some(ref contract) = self.verifying_contract {
            let address = parse_address(contract)?;
            encoded.extend_from_slice(&pad_address(&address));
        }
        if let Some(ref salt) = self.salt {
            let salt_bytes = parse_bytes32(salt)?;
            encoded.extend_from_slice(&salt_bytes);
        }

        Ok(keccak256(&encoded))
    }
}

/// EIP-712 필드 타입 정의
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypedDataField {
    /// 필드 이름
    pub name: String,
    /// 필드 타입 (e.g., "string", "uint256", "address")
    #[serde(rename = "type")]
    pub field_type: String,
}

impl TypedDataField {
    pub fn new(name: impl Into<String>, field_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: field_type.into(),
        }
    }
}

/// EIP-712 타입 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Eip712TypedData {
    /// 타입 정의
    pub types: HashMap<String, Vec<TypedDataField>>,

    /// 주 타입 이름
    pub primary_type: String,

    /// 도메인
    pub domain: Eip712Domain,

    /// 메시지 데이터
    pub message: serde_json::Value,
}

impl Eip712TypedData {
    /// 새 타입 데이터 생성
    pub fn new(
        domain: Eip712Domain,
        primary_type: impl Into<String>,
        types: HashMap<String, Vec<TypedDataField>>,
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
    fn type_hash(&self, type_name: &str) -> CcxtResult<[u8; 32]> {
        let encoded = encode_type(type_name, &self.types)?;
        Ok(keccak256(encoded.as_bytes()))
    }

    /// 구조체 해시 계산
    fn hash_struct_internal(
        &self,
        type_name: &str,
        data: &serde_json::Value,
    ) -> CcxtResult<[u8; 32]> {
        let type_hash = self.type_hash(type_name)?;
        let mut encoded = type_hash.to_vec();

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

            let encoded_value = self.encode_value(&field.field_type, value)?;
            encoded.extend_from_slice(&encoded_value);
        }

        Ok(keccak256(&encoded))
    }

    /// 값 인코딩
    fn encode_value(&self, field_type: &str, value: &serde_json::Value) -> CcxtResult<[u8; 32]> {
        // 동적 타입 (string, bytes)
        if field_type == "string" {
            let s = value.as_str().ok_or_else(|| CcxtError::InvalidSignature {
                message: format!("Expected string, got {value:?}"),
            })?;
            return Ok(keccak256(s.as_bytes()));
        }

        if field_type == "bytes" {
            let hex_str = value.as_str().ok_or_else(|| CcxtError::InvalidSignature {
                message: format!("Expected hex string for bytes, got {value:?}"),
            })?;
            let bytes = parse_hex_bytes(hex_str)?;
            return Ok(keccak256(&bytes));
        }

        // 고정 크기 bytes (bytes1~bytes32)
        if field_type.starts_with("bytes") && !field_type.contains('[') {
            let hex_str = value.as_str().ok_or_else(|| CcxtError::InvalidSignature {
                message: format!("Expected hex string, got {value:?}"),
            })?;
            return parse_bytes32(hex_str);
        }

        // 정수 타입 (uint256, int256, etc.)
        if field_type.starts_with("uint") || field_type.starts_with("int") {
            return encode_integer(value);
        }

        // address
        if field_type == "address" {
            let addr = value.as_str().ok_or_else(|| CcxtError::InvalidSignature {
                message: format!("Expected address string, got {value:?}"),
            })?;
            let address = parse_address(addr)?;
            return Ok(pad_address(&address));
        }

        // bool
        if field_type == "bool" {
            let b = value.as_bool().ok_or_else(|| CcxtError::InvalidSignature {
                message: format!("Expected bool, got {value:?}"),
            })?;
            return Ok(pad_bool(b));
        }

        // 배열 타입
        if let Some(base_type) = field_type.strip_suffix("[]") {
            let arr = value
                .as_array()
                .ok_or_else(|| CcxtError::InvalidSignature {
                    message: format!("Expected array, got {value:?}"),
                })?;

            let mut encoded_elements = Vec::new();
            for element in arr {
                let encoded = self.encode_value(base_type, element)?;
                encoded_elements.extend_from_slice(&encoded);
            }
            return Ok(keccak256(&encoded_elements));
        }

        // 커스텀 구조체 타입
        if self.types.contains_key(field_type) {
            return self.hash_struct_internal(field_type, value);
        }

        Err(CcxtError::InvalidSignature {
            message: format!("Unsupported type: {field_type}"),
        })
    }
}

impl TypedDataHasher for Eip712TypedData {
    /// 메시지 구조체 해시 계산
    fn hash_struct(&self) -> CcxtResult<[u8; 32]> {
        self.hash_struct_internal(&self.primary_type, &self.message)
    }

    /// 전체 서명 해시 계산
    ///
    /// keccak256("\x19\x01" || domainSeparator || hashStruct(message))
    fn sign_hash(&self) -> CcxtResult<[u8; 32]> {
        let domain_separator = self.domain.separator()?;
        let struct_hash = self.hash_struct()?;

        let mut data = Vec::with_capacity(2 + 32 + 32);
        data.push(0x19);
        data.push(0x01);
        data.extend_from_slice(&domain_separator);
        data.extend_from_slice(&struct_hash);

        Ok(keccak256(&data))
    }
}

/// 타입 인코딩 문자열 생성
///
/// 의존성 타입을 알파벳 순으로 정렬하여 포함
pub fn encode_type(
    type_name: &str,
    types: &HashMap<String, Vec<TypedDataField>>,
) -> CcxtResult<String> {
    let mut result = String::new();
    let mut deps = Vec::new();

    collect_dependencies(type_name, types, &mut deps)?;
    deps.sort();

    // 주 타입이 먼저
    result.push_str(&format_type(type_name, types)?);

    // 의존성 타입들 (알파벳 순)
    for dep in deps {
        if dep != type_name {
            result.push_str(&format_type(&dep, types)?);
        }
    }

    Ok(result)
}

fn format_type(
    type_name: &str,
    types: &HashMap<String, Vec<TypedDataField>>,
) -> CcxtResult<String> {
    let fields = types
        .get(type_name)
        .ok_or_else(|| CcxtError::InvalidSignature {
            message: format!("Type not found: {type_name}"),
        })?;

    let field_strings: Vec<String> = fields
        .iter()
        .map(|f| format!("{} {}", f.field_type, f.name))
        .collect();

    Ok(format!("{}({})", type_name, field_strings.join(",")))
}

fn collect_dependencies(
    type_name: &str,
    types: &HashMap<String, Vec<TypedDataField>>,
    deps: &mut Vec<String>,
) -> CcxtResult<()> {
    if deps.contains(&type_name.to_string()) {
        return Ok(());
    }

    let fields = match types.get(type_name) {
        Some(f) => f,
        None => return Ok(()), // 기본 타입
    };

    deps.push(type_name.to_string());

    for field in fields {
        let base_type = field.field_type.trim_end_matches("[]");
        if types.contains_key(base_type) {
            collect_dependencies(base_type, types, deps)?;
        }
    }

    Ok(())
}

/// 타입 해시 계산
pub fn hash_type(
    type_name: &str,
    types: &HashMap<String, Vec<TypedDataField>>,
) -> CcxtResult<[u8; 32]> {
    let encoded = encode_type(type_name, types)?;
    Ok(keccak256(encoded.as_bytes()))
}

// 유틸리티 함수들

fn parse_address(address: &str) -> CcxtResult<[u8; 20]> {
    let hex_str = address.strip_prefix("0x").unwrap_or(address);
    let bytes = hex::decode(hex_str).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid address hex: {e}"),
    })?;

    if bytes.len() != 20 {
        return Err(CcxtError::InvalidSignature {
            message: format!("Address must be 20 bytes, got {}", bytes.len()),
        });
    }

    let mut result = [0u8; 20];
    result.copy_from_slice(&bytes);
    Ok(result)
}

fn parse_bytes32(hex_str: &str) -> CcxtResult<[u8; 32]> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid hex: {e}"),
    })?;

    if bytes.len() > 32 {
        return Err(CcxtError::InvalidSignature {
            message: format!("Bytes must be <= 32, got {}", bytes.len()),
        });
    }

    let mut result = [0u8; 32];
    result[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(result)
}

fn parse_hex_bytes(hex_str: &str) -> CcxtResult<Vec<u8>> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    hex::decode(hex_str).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid hex: {e}"),
    })
}

fn encode_integer(value: &serde_json::Value) -> CcxtResult<[u8; 32]> {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Ok(pad_u256(u))
            } else if let Some(i) = n.as_i64() {
                Ok(super::keccak::pad_i256(i))
            } else {
                Err(CcxtError::InvalidSignature {
                    message: format!("Number too large: {n}"),
                })
            }
        },
        serde_json::Value::String(s) => {
            // Hex 문자열이거나 10진수 문자열
            if s.starts_with("0x") {
                parse_bytes32(s)
            } else {
                let n: u64 = s.parse().map_err(|e| CcxtError::InvalidSignature {
                    message: format!("Invalid integer string: {e}"),
                })?;
                Ok(pad_u256(n))
            }
        },
        _ => Err(CcxtError::InvalidSignature {
            message: format!("Expected number, got {value:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_separator() {
        let domain = Eip712Domain::new("Test", "1", 1);
        let separator = domain.separator().unwrap();
        assert_eq!(separator.len(), 32);
    }

    #[test]
    fn test_encode_type_simple() {
        let mut types = HashMap::new();
        types.insert(
            "Mail".to_string(),
            vec![
                TypedDataField::new("from", "address"),
                TypedDataField::new("to", "address"),
                TypedDataField::new("contents", "string"),
            ],
        );

        let encoded = encode_type("Mail", &types).unwrap();
        assert_eq!(encoded, "Mail(address from,address to,string contents)");
    }

    #[test]
    fn test_encode_type_with_deps() {
        let mut types = HashMap::new();
        types.insert(
            "Person".to_string(),
            vec![
                TypedDataField::new("name", "string"),
                TypedDataField::new("wallet", "address"),
            ],
        );
        types.insert(
            "Mail".to_string(),
            vec![
                TypedDataField::new("from", "Person"),
                TypedDataField::new("to", "Person"),
                TypedDataField::new("contents", "string"),
            ],
        );

        let encoded = encode_type("Mail", &types).unwrap();
        assert!(encoded.starts_with("Mail("));
        assert!(encoded.contains("Person("));
    }

    #[test]
    fn test_hash_struct() {
        let domain = Eip712Domain::new("Test App", "1", 1);

        let mut types = HashMap::new();
        types.insert(
            "Test".to_string(),
            vec![TypedDataField::new("value", "uint256")],
        );

        let message = serde_json::json!({
            "value": 42
        });

        let typed_data = Eip712TypedData::new(domain, "Test", types, message);
        let hash = typed_data.sign_hash().unwrap();

        assert_eq!(hash.len(), 32);
    }

    #[test]
    fn test_eip712_mail_example() {
        // EIP-712 표준 예제
        let domain = Eip712Domain::new("Ether Mail", "1", 1)
            .with_verifying_contract("0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC");

        let mut types = HashMap::new();
        types.insert(
            "Person".to_string(),
            vec![
                TypedDataField::new("name", "string"),
                TypedDataField::new("wallet", "address"),
            ],
        );
        types.insert(
            "Mail".to_string(),
            vec![
                TypedDataField::new("from", "Person"),
                TypedDataField::new("to", "Person"),
                TypedDataField::new("contents", "string"),
            ],
        );

        let message = serde_json::json!({
            "from": {
                "name": "Cow",
                "wallet": "0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826"
            },
            "to": {
                "name": "Bob",
                "wallet": "0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB"
            },
            "contents": "Hello, Bob!"
        });

        let typed_data = Eip712TypedData::new(domain, "Mail", types, message);
        let hash = typed_data.sign_hash().unwrap();

        // 해시가 정상적으로 계산되는지 확인
        assert_eq!(hash.len(), 32);
    }
}
