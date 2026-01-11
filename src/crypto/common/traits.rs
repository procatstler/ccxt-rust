//! Common cryptographic traits for DEX signing
//!
//! 이 모듈은 DEX 거래소 인증에 필요한 공통 트레이트를 정의합니다.

use crate::errors::CcxtResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// ECDSA 서명 결과
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// r 값 (32 bytes)
    pub r: [u8; 32],
    /// s 값 (32 bytes)
    pub s: [u8; 32],
    /// v 값 (recovery id, 27 또는 28)
    pub v: u8,
}

impl Signature {
    /// 새 서명 생성
    pub fn new(r: [u8; 32], s: [u8; 32], v: u8) -> Self {
        Self { r, s, v }
    }

    /// 65바이트 형식으로 변환 (r || s || v)
    pub fn to_bytes(&self) -> [u8; 65] {
        let mut bytes = [0u8; 65];
        bytes[..32].copy_from_slice(&self.r);
        bytes[32..64].copy_from_slice(&self.s);
        bytes[64] = self.v;
        bytes
    }

    /// 65바이트에서 파싱
    pub fn from_bytes(bytes: &[u8; 65]) -> Self {
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&bytes[..32]);
        s.copy_from_slice(&bytes[32..64]);
        Self { r, s, v: bytes[64] }
    }

    /// Hex 문자열로 변환 (0x 접두사 포함)
    pub fn to_hex(&self) -> String {
        format!("0x{}", hex::encode(self.to_bytes()))
    }

    /// Hex 문자열에서 파싱 (0x 접두사 선택)
    pub fn from_hex(hex_str: &str) -> CcxtResult<Self> {
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let bytes =
            hex::decode(hex_str).map_err(|e| crate::errors::CcxtError::InvalidSignature {
                message: format!("Invalid hex: {e}"),
            })?;

        if bytes.len() != 65 {
            return Err(crate::errors::CcxtError::InvalidSignature {
                message: format!("Expected 65 bytes, got {}", bytes.len()),
            });
        }

        let mut arr = [0u8; 65];
        arr.copy_from_slice(&bytes);
        Ok(Self::from_bytes(&arr))
    }

    /// r, s를 개별 hex 문자열로 반환
    pub fn r_hex(&self) -> String {
        format!("0x{}", hex::encode(self.r))
    }

    pub fn s_hex(&self) -> String {
        format!("0x{}", hex::encode(self.s))
    }
}

/// 서명자 트레이트 - DEX 서명의 공통 인터페이스
#[async_trait]
pub trait Signer: Send + Sync {
    /// 지갑 주소 반환 (체크섬 형식)
    fn address(&self) -> &str;

    /// 원시 메시지 서명 (Ethereum personal_sign 스타일)
    ///
    /// 메시지에 "\x19Ethereum Signed Message:\n{len}" 접두사를 추가하고 서명
    async fn sign_message(&self, message: &[u8]) -> CcxtResult<Signature>;

    /// 해시된 데이터 직접 서명 (접두사 없음)
    async fn sign_hash(&self, hash: &[u8; 32]) -> CcxtResult<Signature>;
}

/// 타입 데이터 해싱 트레이트
///
/// EIP-712 및 StarkNet 타입 데이터 해싱을 위한 공통 인터페이스
pub trait TypedDataHasher {
    /// 구조화된 데이터의 해시 계산
    fn hash_struct(&self) -> CcxtResult<[u8; 32]>;

    /// 전체 서명 해시 계산 (도메인 분리자 포함)
    fn sign_hash(&self) -> CcxtResult<[u8; 32]>;
}

/// 인증 토큰 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    /// JWT 토큰 문자열
    pub token: String,
    /// 만료 시간 (Unix timestamp, 초)
    pub expires_at: i64,
}

#[allow(dead_code)]
impl AuthToken {
    /// 새 인증 토큰 생성
    pub fn new(token: String, expires_at: i64) -> Self {
        Self { token, expires_at }
    }

    /// 토큰이 만료되었는지 확인
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        now >= self.expires_at
    }

    /// 만료까지 남은 시간 (초)
    pub fn remaining_seconds(&self) -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        self.expires_at - now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_bytes_roundtrip() {
        let sig = Signature::new([1u8; 32], [2u8; 32], 27);

        let bytes = sig.to_bytes();
        let recovered = Signature::from_bytes(&bytes);

        assert_eq!(sig, recovered);
    }

    #[test]
    fn test_signature_hex_roundtrip() {
        let sig = Signature::new([0xab; 32], [0xcd; 32], 28);

        let hex_str = sig.to_hex();
        let recovered = Signature::from_hex(&hex_str).unwrap();

        assert_eq!(sig, recovered);
    }

    #[test]
    fn test_auth_token_expiry() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // 만료된 토큰
        let expired = AuthToken::new("token".to_string(), now - 100);
        assert!(expired.is_expired());

        // 유효한 토큰
        let valid = AuthToken::new("token".to_string(), now + 100);
        assert!(!valid.is_expired());
        assert!(valid.remaining_seconds() > 0);
    }
}
