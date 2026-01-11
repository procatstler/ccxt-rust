//! DEX Cryptographic Utilities
//!
//! 이 모듈은 DEX(탈중앙화 거래소) 인증 및 서명을 위한 암호화 기능을 제공합니다.
//!
//! **Note**: This module requires the `dex` feature to be enabled.
//!
//! ```toml
//! [dependencies]
//! ccxt-rust = { version = "0.1", features = ["dex"] }
//! ```
//!
//! # 모듈 구조
//!
//! - `common`: 공통 트레이트 및 유틸리티
//! - `evm`: EVM 호환 체인용 (Keccak256, EIP-712, secp256k1)
//! - `starknet`: StarkNet 체인용 (Poseidon, SNIP-12, StarkNet 곡선)
//! - `cosmos`: Cosmos SDK 체인용 (BIP-44, Bech32, secp256k1)
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
//!
//! ## Cosmos SDK (dYdX v4)
//!
//! ```rust,ignore
//! use ccxt_rust::crypto::cosmos::{CosmosWallet, DYDX_MAINNET};
//!
//! // 니모닉에서 dYdX 지갑 생성
//! let wallet = CosmosWallet::from_mnemonic(&mnemonic, &DYDX_MAINNET, 0)?;
//!
//! // 주소 확인
//! println!("Address: {}", wallet.address());
//!
//! // 메시지 서명
//! let signature = wallet.sign_bytes(b"Hello, dYdX!")?;
//! ```

pub mod common;
#[cfg(feature = "dex")]
pub mod cosmos;
#[cfg(feature = "dex")]
pub mod evm;
#[cfg(feature = "dex")]
pub mod starknet;

// Re-exports: Common
pub use common::{Signature, Signer, TypedDataHasher};

// Re-exports: EVM (requires "dex" feature)
#[cfg(feature = "dex")]
pub use evm::{
    keccak256, keccak256_hash, Eip712Domain, Eip712TypedData, EvmWallet, TypedDataField,
};

// Re-exports: StarkNet (requires "dex" feature)
#[cfg(feature = "dex")]
pub use starknet::{
    compute_starknet_address, derive_starknet_private_key, encode_typed_data_hash,
    get_public_key as starknet_get_public_key, pedersen_hash, poseidon_hash, poseidon_hash_many,
    sign_hash as starknet_sign_hash, verify_signature as starknet_verify_signature, ParadexOrder,
    StarkNetAccount, StarkNetDomain, StarkNetSignature, StarkNetTypedData, StarkNetTypedDataField,
    StarkNetWallet,
};

// Re-exports: Cosmos (requires "dex" feature)
#[cfg(feature = "dex")]
pub use cosmos::{
    // Keys
    derive_private_key as cosmos_derive_private_key,
    mnemonic_to_seed,
    private_key_to_public_key as cosmos_public_key,
    // Address
    public_key_to_address as cosmos_address,
    sign_amino,
    // Signer
    sign_bytes as cosmos_sign_bytes,
    ChainConfig,
    CosmosKeyPair,
    CosmosSignature,
    // Wallet
    CosmosWallet,
    COSMOS_HUB,
    DYDX_MAINNET,
    DYDX_TESTNET,
    INJECTIVE,
    OSMOSIS,
};
