//! DEX Cryptographic Utilities
//!
//! 이 모듈은 DEX(탈중앙화 거래소) 인증 및 서명을 위한 암호화 기능을 제공합니다.
//!
//! # 모듈 구조
//!
//! - `common`: 공통 트레이트 및 유틸리티
//! - `evm`: EVM 호환 체인용 (Keccak256, EIP-712, secp256k1)
//! - `starknet`: StarkNet 체인용 (Poseidon, SNIP-12, StarkNet 곡선)
//!
//! # 사용 예시
//!
//! ## EVM (Hyperliquid)
//!
//! ```rust,ignore
//! use ccxt_rust::crypto::evm::{EvmWallet, Eip712TypedData};
//!
//! // EVM 지갑 생성
//! let wallet = EvmWallet::from_private_key("0x...")?;
//!
//! // EIP-712 타입 데이터 서명
//! let signature = wallet.sign_typed_data(&typed_data)?;
//! ```
//!
//! ## StarkNet (Paradex)
//!
//! ```rust,ignore
//! use ccxt_rust::crypto::starknet::{StarkNetWallet, ParadexOrder};
//!
//! // ETH 개인키에서 StarkNet 지갑 파생
//! let wallet = StarkNetWallet::from_eth_private_key(&eth_key, "paradex")?;
//!
//! // 주문 서명
//! let order = ParadexOrder::new("ETH-USD", "Buy", "Limit", "1.0");
//! let (r, s) = wallet.sign_order(&order, "SN_MAIN")?;
//! ```

pub mod common;
pub mod evm;
pub mod starknet;

// Re-exports: Common
pub use common::{Signature, Signer, TypedDataHasher};

// Re-exports: EVM
pub use evm::{
    keccak256, keccak256_hash,
    Eip712Domain, Eip712TypedData, TypedDataField,
    EvmWallet,
};

// Re-exports: StarkNet
pub use starknet::{
    poseidon_hash, poseidon_hash_many, pedersen_hash,
    sign_hash as starknet_sign_hash, verify_signature as starknet_verify_signature,
    get_public_key as starknet_get_public_key,
    StarkNetSignature,
    StarkNetDomain, StarkNetTypedData, StarkNetTypedDataField, encode_typed_data_hash,
    StarkNetAccount, derive_starknet_private_key, compute_starknet_address,
    StarkNetWallet, ParadexOrder,
};
