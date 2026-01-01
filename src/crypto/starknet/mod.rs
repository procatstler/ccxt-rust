//! StarkNet Cryptography Module
//!
//! StarkNet L2 체인을 위한 암호화 유틸리티를 제공합니다.
//!
//! # 주요 기능
//!
//! - Poseidon 해싱
//! - Pedersen 해싱
//! - StarkNet ECDSA 서명
//! - 계정 파생 (ETH → StarkNet)
//! - 타입 데이터 서명 (SNIP-12)
//! - Paradex 거래소 인증

mod poseidon;
mod curve;
mod typed_data;
mod account;
mod wallet;
pub mod paradex;

pub use poseidon::{poseidon_hash, poseidon_hash_many, pedersen_hash};
pub use curve::{sign_hash, verify_signature, get_public_key, StarkNetSignature};
pub use typed_data::{StarkNetDomain, StarkNetTypedData, StarkNetTypedDataField, encode_typed_data_hash};
pub use account::{StarkNetAccount, derive_starknet_private_key, compute_starknet_address};
pub use wallet::{StarkNetWallet, ParadexOrder};
pub use paradex::{
    ParadexAuthMessage, ParadexOnboardingMessage, ParadexOrderMessage, ParadexFullNodeMessage,
    PARADEX_CHAIN_ID_MAINNET, PARADEX_CHAIN_ID_TESTNET,
};
