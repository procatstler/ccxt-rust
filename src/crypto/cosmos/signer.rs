//! Cosmos SDK Transaction Signing
//!
//! Cosmos SDK 체인을 위한 트랜잭션 서명 기능을 제공합니다.
//!
//! # 서명 방식
//!
//! Cosmos SDK는 두 가지 서명 방식을 지원합니다:
//! - Direct (Protobuf): 권장 방식, 더 효율적
//! - Amino (Legacy): 레거시 호환성
//!
//! # 참조
//!
//! - [Cosmos SDK Signing](https://docs.cosmos.network/main/core/encoding)
//! - [ADR-036: Arbitrary Signature](https://github.com/cosmos/cosmos-sdk/blob/main/docs/architecture/adr-036-arbitrary-signature.md)

use crate::errors::{CcxtError, CcxtResult};
use k256::ecdsa::{
    signature::Signer as K256Signer,
    SigningKey, VerifyingKey, Signature as K256Signature,
    signature::Verifier,
};
use sha2::{Sha256, Digest};

/// Cosmos ECDSA 서명 (r, s)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosmosSignature {
    /// r 값 (32 bytes)
    pub r: [u8; 32],
    /// s 값 (32 bytes)
    pub s: [u8; 32],
}

impl CosmosSignature {
    /// 새 서명 생성
    pub fn new(r: [u8; 32], s: [u8; 32]) -> Self {
        Self { r, s }
    }

    /// 64바이트 형식으로 변환 (r || s)
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(&self.r);
        bytes[32..].copy_from_slice(&self.s);
        bytes
    }

    /// 64바이트에서 파싱
    pub fn from_bytes(bytes: &[u8]) -> CcxtResult<Self> {
        if bytes.len() != 64 {
            return Err(CcxtError::InvalidSignature {
                message: format!("Expected 64 bytes, got {}", bytes.len()),
            });
        }

        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&bytes[..32]);
        s.copy_from_slice(&bytes[32..]);

        Ok(Self { r, s })
    }

    /// Base64 인코딩
    pub fn to_base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(self.to_bytes())
    }

    /// Base64 디코딩
    pub fn from_base64(encoded: &str) -> CcxtResult<Self> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)
            .map_err(|e| CcxtError::InvalidSignature {
                message: format!("Invalid base64: {}", e),
            })?;
        Self::from_bytes(&bytes)
    }

    /// Hex 인코딩
    pub fn to_hex(&self) -> String {
        hex::encode(self.to_bytes())
    }

    /// Hex 디코딩
    pub fn from_hex(hex_str: &str) -> CcxtResult<Self> {
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let bytes = hex::decode(hex_str)
            .map_err(|e| CcxtError::InvalidSignature {
                message: format!("Invalid hex: {}", e),
            })?;
        Self::from_bytes(&bytes)
    }
}

/// 바이트 데이터 서명 (SHA256 해시 후 서명)
///
/// # Arguments
///
/// * `private_key` - 32바이트 개인키
/// * `data` - 서명할 데이터
///
/// # Returns
///
/// CosmosSignature (r, s)
pub fn sign_bytes(private_key: &[u8; 32], data: &[u8]) -> CcxtResult<CosmosSignature> {
    // SHA-256 해시
    let hash = Sha256::digest(data);
    sign_hash(private_key, hash.as_slice())
}

/// 해시 직접 서명
///
/// # Arguments
///
/// * `private_key` - 32바이트 개인키
/// * `hash` - 32바이트 해시
///
/// # Returns
///
/// CosmosSignature (r, s)
pub fn sign_hash(private_key: &[u8; 32], hash: &[u8]) -> CcxtResult<CosmosSignature> {
    let signing_key = SigningKey::from_bytes(private_key.into())
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid private key: {}", e),
        })?;

    let signature: K256Signature = signing_key.sign(hash);
    let bytes = signature.to_bytes();

    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r.copy_from_slice(&bytes[..32]);
    s.copy_from_slice(&bytes[32..]);

    Ok(CosmosSignature::new(r, s))
}

/// Amino 스타일 서명 (ADR-036)
///
/// Amino JSON 인코딩된 메시지를 서명합니다.
/// 주로 오프체인 메시지 서명에 사용됩니다.
///
/// # Arguments
///
/// * `private_key` - 32바이트 개인키
/// * `chain_id` - 체인 ID
/// * `signer` - 서명자 주소
/// * `data` - 서명할 데이터 (일반적으로 Base64 인코딩)
///
/// # Returns
///
/// CosmosSignature
pub fn sign_amino(
    private_key: &[u8; 32],
    chain_id: &str,
    signer: &str,
    data: &str,
) -> CcxtResult<CosmosSignature> {
    // ADR-036 Amino JSON 형식
    // {
    //   "account_number": "0",
    //   "chain_id": "<chain_id>",
    //   "fee": { "amount": [], "gas": "0" },
    //   "memo": "",
    //   "msgs": [
    //     {
    //       "type": "sign/MsgSignData",
    //       "value": {
    //         "data": "<base64_data>",
    //         "signer": "<signer_address>"
    //       }
    //     }
    //   ],
    //   "sequence": "0"
    // }

    let sign_doc = serde_json::json!({
        "account_number": "0",
        "chain_id": chain_id,
        "fee": {
            "amount": [],
            "gas": "0"
        },
        "memo": "",
        "msgs": [{
            "type": "sign/MsgSignData",
            "value": {
                "data": data,
                "signer": signer
            }
        }],
        "sequence": "0"
    });

    // 정렬된 JSON (Amino 요구사항)
    let json_bytes = serde_json::to_vec(&sign_doc)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("JSON serialization failed: {}", e),
        })?;

    sign_bytes(private_key, &json_bytes)
}

/// 서명 검증
///
/// # Arguments
///
/// * `public_key` - 33바이트 압축 공개키
/// * `data` - 원본 데이터
/// * `signature` - 검증할 서명
///
/// # Returns
///
/// 검증 성공 여부
pub fn verify_signature(
    public_key: &[u8; 33],
    data: &[u8],
    signature: &CosmosSignature,
) -> CcxtResult<bool> {
    let verifying_key = VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid public key: {}", e),
        })?;

    // SHA-256 해시
    let hash = Sha256::digest(data);

    // 서명 복원
    let sig_bytes = signature.to_bytes();
    let k256_sig = K256Signature::from_slice(&sig_bytes)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid signature format: {}", e),
        })?;

    Ok(verifying_key.verify(&hash, &k256_sig).is_ok())
}

/// 해시에 대한 서명 검증
pub fn verify_hash_signature(
    public_key: &[u8; 33],
    hash: &[u8],
    signature: &CosmosSignature,
) -> CcxtResult<bool> {
    let verifying_key = VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid public key: {}", e),
        })?;

    let sig_bytes = signature.to_bytes();
    let k256_sig = K256Signature::from_slice(&sig_bytes)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid signature format: {}", e),
        })?;

    Ok(verifying_key.verify(hash, &k256_sig).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::cosmos::keys::{derive_private_key, coin_type};

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_sign_and_verify() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let message = b"Hello, Cosmos!";

        // 서명
        let signature = sign_bytes(&keypair.private_key, message).unwrap();

        // 검증
        let is_valid = verify_signature(&keypair.public_key, message, &signature).unwrap();
        assert!(is_valid);

        // 다른 메시지로 검증 실패
        let is_valid = verify_signature(&keypair.public_key, b"Wrong message", &signature).unwrap();
        assert!(!is_valid);
    }

    #[test]
    fn test_signature_serialization() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let signature = sign_bytes(&keypair.private_key, b"test").unwrap();

        // Bytes roundtrip
        let bytes = signature.to_bytes();
        let recovered = CosmosSignature::from_bytes(&bytes).unwrap();
        assert_eq!(signature, recovered);

        // Base64 roundtrip
        let base64 = signature.to_base64();
        let recovered = CosmosSignature::from_base64(&base64).unwrap();
        assert_eq!(signature, recovered);

        // Hex roundtrip
        let hex = signature.to_hex();
        let recovered = CosmosSignature::from_hex(&hex).unwrap();
        assert_eq!(signature, recovered);
    }

    #[test]
    fn test_sign_amino() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();

        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"test data");

        let signature = sign_amino(
            &keypair.private_key,
            "dydx-testnet-4",
            "dydx1abc...",
            &data,
        ).unwrap();

        // 서명이 64바이트인지 확인
        assert_eq!(signature.to_bytes().len(), 64);
    }

    #[test]
    fn test_deterministic_signature() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let message = b"deterministic test";

        // k256 ECDSA는 RFC 6979 결정적 서명 사용
        let sig1 = sign_bytes(&keypair.private_key, message).unwrap();
        let sig2 = sign_bytes(&keypair.private_key, message).unwrap();

        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_different_keys_different_signatures() {
        let keypair1 = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let keypair2 = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 1).unwrap();
        let message = b"test message";

        let sig1 = sign_bytes(&keypair1.private_key, message).unwrap();
        let sig2 = sign_bytes(&keypair2.private_key, message).unwrap();

        assert_ne!(sig1, sig2);

        // 각 서명은 해당 키로만 검증 가능
        assert!(verify_signature(&keypair1.public_key, message, &sig1).unwrap());
        assert!(!verify_signature(&keypair2.public_key, message, &sig1).unwrap());
    }
}
