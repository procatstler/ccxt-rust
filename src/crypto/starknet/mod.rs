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

mod account;
mod curve;
pub mod paradex;
mod poseidon;
mod typed_data;
mod wallet;

pub use account::{compute_starknet_address, derive_starknet_private_key, StarkNetAccount};
pub use curve::{get_public_key, sign_hash, verify_signature, StarkNetSignature};
pub use paradex::{
    ParadexAuthMessage, ParadexFullNodeMessage, ParadexOnboardingMessage, ParadexOrderMessage,
    PARADEX_CHAIN_ID_MAINNET, PARADEX_CHAIN_ID_TESTNET,
};
pub use poseidon::{pedersen_hash, poseidon_hash, poseidon_hash_many};
pub use typed_data::{
    encode_typed_data_hash, StarkNetDomain, StarkNetTypedData, StarkNetTypedDataField,
};
pub use wallet::{ParadexOrder, StarkNetWallet};
