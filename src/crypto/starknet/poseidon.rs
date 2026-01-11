//! Poseidon Hash Implementation
//!
//! StarkNet에서 사용하는 Poseidon 해시 함수를 제공합니다.
//!
//! # 참조
//!
//! - [StarkNet Poseidon Hash](https://docs.starknet.io/documentation/architecture_and_concepts/Hashing/hash-functions/)

#![allow(dead_code)]

use starknet_crypto::{
    pedersen_hash as stark_pedersen_hash, poseidon_hash as stark_poseidon_hash,
    poseidon_hash_many as stark_poseidon_hash_many,
};
use starknet_types_core::felt::Felt;

/// Poseidon 해시 (2개 입력)
///
/// # Arguments
///
/// * `x` - 첫 번째 필드 요소
/// * `y` - 두 번째 필드 요소
///
/// # Returns
///
/// 해시 결과
pub fn poseidon_hash(x: &Felt, y: &Felt) -> Felt {
    stark_poseidon_hash(*x, *y)
}

/// Poseidon 해시 (여러 입력)
///
/// # Arguments
///
/// * `values` - 해시할 필드 요소들
///
/// # Returns
///
/// 해시 결과
pub fn poseidon_hash_many(values: &[Felt]) -> Felt {
    stark_poseidon_hash_many(values)
}

/// Pedersen 해시 (2개 입력)
///
/// # Arguments
///
/// * `x` - 첫 번째 필드 요소
/// * `y` - 두 번째 필드 요소
///
/// # Returns
///
/// 해시 결과
pub fn pedersen_hash(x: &Felt, y: &Felt) -> Felt {
    stark_pedersen_hash(x, y)
}

/// 바이트 배열을 Felt로 변환
///
/// # Arguments
///
/// * `bytes` - 변환할 바이트 배열 (최대 32바이트)
///
/// # Returns
///
/// Felt 값
pub fn bytes_to_felt(bytes: &[u8]) -> Felt {
    let mut padded = [0u8; 32];
    let start = 32 - bytes.len().min(32);
    padded[start..].copy_from_slice(&bytes[..bytes.len().min(32)]);
    Felt::from_bytes_be(&padded)
}

/// Felt를 바이트 배열로 변환
///
/// # Arguments
///
/// * `felt` - 변환할 Felt 값
///
/// # Returns
///
/// 32바이트 배열
pub fn felt_to_bytes(felt: &Felt) -> [u8; 32] {
    felt.to_bytes_be()
}

/// 문자열을 Felt로 변환 (short string encoding)
///
/// StarkNet short string은 최대 31바이트 문자열을 Felt로 인코딩
///
/// # Arguments
///
/// * `s` - 변환할 문자열 (최대 31자)
///
/// # Returns
///
/// Felt 값
pub fn string_to_felt(s: &str) -> Felt {
    let bytes = s.as_bytes();
    let len = bytes.len().min(31);
    let mut padded = [0u8; 32];
    padded[32 - len..].copy_from_slice(&bytes[..len]);
    Felt::from_bytes_be(&padded)
}

/// u64를 Felt로 변환
pub fn u64_to_felt(n: u64) -> Felt {
    Felt::from(n)
}

/// u128을 Felt로 변환
pub fn u128_to_felt(n: u128) -> Felt {
    Felt::from(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poseidon_hash() {
        let x = Felt::from(1u64);
        let y = Felt::from(2u64);
        let result = poseidon_hash(&x, &y);

        // Poseidon 해시는 결정적이어야 함
        let result2 = poseidon_hash(&x, &y);
        assert_eq!(result, result2);
    }

    #[test]
    fn test_poseidon_hash_many() {
        let values = vec![Felt::from(1u64), Felt::from(2u64), Felt::from(3u64)];
        let result = poseidon_hash_many(&values);

        // 결과가 유효한 Felt여야 함
        assert_ne!(result, Felt::ZERO);
    }

    #[test]
    fn test_pedersen_hash() {
        let x = Felt::from(1u64);
        let y = Felt::from(2u64);
        let result = pedersen_hash(&x, &y);

        // Pedersen 해시는 결정적이어야 함
        let result2 = pedersen_hash(&x, &y);
        assert_eq!(result, result2);
    }

    #[test]
    fn test_bytes_to_felt() {
        let bytes = [1u8, 2, 3, 4];
        let felt = bytes_to_felt(&bytes);

        // 변환이 성공해야 함
        assert_ne!(felt, Felt::ZERO);
    }

    #[test]
    fn test_string_to_felt() {
        let s = "hello";
        let felt = string_to_felt(s);

        // 변환이 성공해야 함
        assert_ne!(felt, Felt::ZERO);

        // 동일한 문자열은 동일한 Felt를 생성해야 함
        let felt2 = string_to_felt(s);
        assert_eq!(felt, felt2);
    }

    #[test]
    fn test_u64_to_felt() {
        let n: u64 = 12345;
        let felt = u64_to_felt(n);

        assert_ne!(felt, Felt::ZERO);
    }
}
