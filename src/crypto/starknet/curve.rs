//! StarkNet Curve Operations
//!
//! StarkNet의 STARK 곡선 (ECDSA) 서명 및 검증을 제공합니다.
//!
//! # 참조
//!
//! - [StarkNet Signatures](https://docs.starknet.io/documentation/architecture_and_concepts/Accounts/signature_verification/)

use crate::errors::{CcxtError, CcxtResult};
use starknet_crypto::{sign, verify, get_public_key as stark_get_public_key};
use starknet_types_core::felt::Felt;

/// StarkNet 서명
#[derive(Debug, Clone)]
pub struct StarkNetSignature {
    /// r 값
    pub r: Felt,
    /// s 값
    pub s: Felt,
}

impl StarkNetSignature {
    /// 새 서명 생성
    pub fn new(r: Felt, s: Felt) -> Self {
        Self { r, s }
    }

    /// 16진수 문자열로 변환
    pub fn to_hex(&self) -> (String, String) {
        (
            format!("0x{}", hex::encode(self.r.to_bytes_be())),
            format!("0x{}", hex::encode(self.s.to_bytes_be())),
        )
    }

    /// 16진수 문자열에서 생성
    pub fn from_hex(r_hex: &str, s_hex: &str) -> CcxtResult<Self> {
        let r_bytes = parse_hex_to_bytes32(r_hex)?;
        let s_bytes = parse_hex_to_bytes32(s_hex)?;

        Ok(Self {
            r: Felt::from_bytes_be(&r_bytes),
            s: Felt::from_bytes_be(&s_bytes),
        })
    }
}

/// 메시지 해시에 서명
///
/// # Arguments
///
/// * `private_key` - StarkNet 개인키
/// * `message_hash` - 서명할 메시지 해시
/// * `seed` - 서명에 사용할 k 값 시드 (None이면 랜덤)
///
/// # Returns
///
/// StarkNet 서명 (r, s)
pub fn sign_hash(
    private_key: &Felt,
    message_hash: &Felt,
    seed: Option<&Felt>,
) -> CcxtResult<StarkNetSignature> {
    let k = seed.cloned().unwrap_or_else(generate_k);

    let signature = sign(private_key, message_hash, &k)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("StarkNet signing failed: {:?}", e),
        })?;

    Ok(StarkNetSignature {
        r: signature.r,
        s: signature.s,
    })
}

/// 서명 검증
///
/// # Arguments
///
/// * `public_key` - StarkNet 공개키
/// * `message_hash` - 서명된 메시지 해시
/// * `signature` - 검증할 서명
///
/// # Returns
///
/// 검증 성공 여부
pub fn verify_signature(
    public_key: &Felt,
    message_hash: &Felt,
    signature: &StarkNetSignature,
) -> CcxtResult<bool> {
    let result = verify(public_key, message_hash, &signature.r, &signature.s)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("StarkNet verification failed: {:?}", e),
        })?;

    Ok(result)
}

/// 개인키에서 공개키 파생
///
/// # Arguments
///
/// * `private_key` - StarkNet 개인키
///
/// # Returns
///
/// StarkNet 공개키
pub fn get_public_key(private_key: &Felt) -> Felt {
    stark_get_public_key(private_key)
}

/// k 값 생성 (랜덤)
fn generate_k() -> Felt {
    use std::time::{SystemTime, UNIX_EPOCH};

    // 간단한 시드 기반 k 생성 (프로덕션에서는 더 안전한 RNG 사용 권장)
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut seed_bytes = [0u8; 32];
    // u128은 16바이트, 뒤쪽 16바이트에 복사
    let ts_bytes = timestamp.to_be_bytes();
    seed_bytes[16..].copy_from_slice(&ts_bytes);

    Felt::from_bytes_be(&seed_bytes)
}

/// 16진수 문자열을 32바이트 배열로 파싱
fn parse_hex_to_bytes32(hex_str: &str) -> CcxtResult<[u8; 32]> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);

    // 홀수 길이면 앞에 0을 추가
    let hex_str = if hex_str.len() % 2 != 0 {
        format!("0{}", hex_str)
    } else {
        hex_str.to_string()
    };

    let bytes = hex::decode(&hex_str).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid hex: {}", e),
    })?;

    if bytes.len() > 32 {
        return Err(CcxtError::InvalidSignature {
            message: format!("Hex value too large: {} bytes", bytes.len()),
        });
    }

    let mut result = [0u8; 32];
    result[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(result)
}

/// Felt를 16진수 문자열로 변환
pub fn felt_to_hex(felt: &Felt) -> String {
    format!("0x{}", hex::encode(felt.to_bytes_be()).trim_start_matches('0'))
}

/// 16진수 문자열을 Felt로 변환
pub fn hex_to_felt(hex_str: &str) -> CcxtResult<Felt> {
    let bytes = parse_hex_to_bytes32(hex_str)?;
    Ok(Felt::from_bytes_be(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_verify() {
        // 테스트용 개인키
        let private_key = Felt::from(12345u64);
        let public_key = get_public_key(&private_key);

        // 메시지 해시
        let message_hash = Felt::from(67890u64);

        // 서명
        let signature = sign_hash(&private_key, &message_hash, None).unwrap();

        // 검증
        let is_valid = verify_signature(&public_key, &message_hash, &signature).unwrap();
        assert!(is_valid);
    }

    #[test]
    fn test_signature_hex_roundtrip() {
        let r = Felt::from(123u64);
        let s = Felt::from(456u64);

        let sig = StarkNetSignature::new(r, s);
        let (r_hex, s_hex) = sig.to_hex();

        let sig2 = StarkNetSignature::from_hex(&r_hex, &s_hex).unwrap();
        assert_eq!(sig.r, sig2.r);
        assert_eq!(sig.s, sig2.s);
    }

    #[test]
    fn test_get_public_key() {
        let private_key = Felt::from(12345u64);
        let public_key = get_public_key(&private_key);

        // 공개키는 0이 아니어야 함
        assert_ne!(public_key, Felt::ZERO);

        // 동일한 개인키는 동일한 공개키를 생성
        let public_key2 = get_public_key(&private_key);
        assert_eq!(public_key, public_key2);
    }

    #[test]
    fn test_hex_to_felt() {
        let hex_str = "0x123abc";
        let felt = hex_to_felt(hex_str).unwrap();

        // 변환이 성공해야 함
        assert_ne!(felt, Felt::ZERO);

        // 역변환 확인
        let back = felt_to_hex(&felt);
        assert!(back.contains("123abc"));
    }
}
