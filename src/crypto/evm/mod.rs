//! EVM-compatible cryptographic utilities
//!
//! Ethereum 및 EVM 호환 체인을 위한 암호화 기능을 제공합니다.
//!
//! # 모듈
//!
//! - `keccak`: Keccak256 해싱
//! - `secp256k1`: ECDSA 서명
//! - `eip712`: EIP-712 타입 데이터 인코딩
//! - `wallet`: EVM 지갑 관리

mod eip712;
mod keccak;
mod secp256k1;
mod wallet;

pub use eip712::{encode_type, hash_type, Eip712Domain, Eip712TypedData, TypedDataField};
pub use keccak::{keccak256, keccak256_hash};
pub use secp256k1::{private_key_to_address, recover_address, sign_hash};
pub use wallet::EvmWallet;
