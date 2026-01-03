//! Cosmos Wallet Management
//!
//! Cosmos SDK 체인을 위한 고수준 지갑 관리 기능을 제공합니다.
//!
//! # 사용 예시
//!
//! ```rust,ignore
//! use ccxt_rust::crypto::cosmos::{CosmosWallet, DYDX_MAINNET};
//!
//! // 니모닉에서 지갑 생성
//! let wallet = CosmosWallet::from_mnemonic(
//!     "abandon abandon ... about",
//!     &DYDX_MAINNET,
//!     0,
//! )?;
//!
//! // 주소 확인
//! println!("Address: {}", wallet.address());
//!
//! // 메시지 서명
//! let signature = wallet.sign_bytes(b"Hello, dYdX!")?;
//! ```

use crate::errors::CcxtResult;
use super::keys::{derive_private_key, parse_private_key, CosmosKeyPair};
use super::address::{public_key_to_address, ChainConfig};
use super::signer::{sign_bytes, sign_amino, verify_signature, CosmosSignature};

/// Cosmos 지갑
///
/// 키 쌍과 체인 설정을 관리하는 고수준 인터페이스입니다.
pub struct CosmosWallet {
    /// 키 쌍 (개인키 + 공개키)
    keypair: CosmosKeyPair,
    /// 체인 설정
    config: ChainConfig,
    /// Bech32 주소 (캐시)
    address: String,
}

impl CosmosWallet {
    /// 니모닉에서 지갑 생성
    ///
    /// # Arguments
    ///
    /// * `mnemonic` - BIP-39 니모닉 문구 (12/24 단어)
    /// * `config` - 체인 설정
    /// * `index` - 주소 인덱스 (일반적으로 0)
    ///
    /// # Returns
    ///
    /// CosmosWallet 인스턴스
    pub fn from_mnemonic(
        mnemonic: &str,
        config: &ChainConfig,
        index: u32,
    ) -> CcxtResult<Self> {
        let keypair = derive_private_key(mnemonic, config.coin_type, index)?;
        let address = public_key_to_address(&keypair.public_key, config.address_prefix)?;

        Ok(Self {
            keypair,
            config: config.clone(),
            address,
        })
    }

    /// 개인키에서 지갑 생성
    ///
    /// # Arguments
    ///
    /// * `private_key` - 32바이트 개인키
    /// * `config` - 체인 설정
    ///
    /// # Returns
    ///
    /// CosmosWallet 인스턴스
    pub fn from_private_key(
        private_key: [u8; 32],
        config: &ChainConfig,
    ) -> CcxtResult<Self> {
        let keypair = CosmosKeyPair::from_private_key(private_key)?;
        let address = public_key_to_address(&keypair.public_key, config.address_prefix)?;

        Ok(Self {
            keypair,
            config: config.clone(),
            address,
        })
    }

    /// Hex 개인키에서 지갑 생성
    ///
    /// # Arguments
    ///
    /// * `hex_key` - Hex 인코딩된 개인키 (0x 접두사 선택)
    /// * `config` - 체인 설정
    ///
    /// # Returns
    ///
    /// CosmosWallet 인스턴스
    pub fn from_private_key_hex(
        hex_key: &str,
        config: &ChainConfig,
    ) -> CcxtResult<Self> {
        let private_key = parse_private_key(hex_key)?;
        Self::from_private_key(private_key, config)
    }

    /// 지갑 주소 반환
    pub fn address(&self) -> &str {
        &self.address
    }

    /// 공개키 반환 (압축, 33바이트)
    pub fn public_key(&self) -> &[u8; 33] {
        &self.keypair.public_key
    }

    /// 공개키 hex 반환
    pub fn public_key_hex(&self) -> String {
        self.keypair.public_key_hex()
    }

    /// 체인 설정 반환
    pub fn config(&self) -> &ChainConfig {
        &self.config
    }

    /// 바이트 데이터 서명
    ///
    /// 데이터를 SHA-256 해시하고 ECDSA 서명합니다.
    ///
    /// # Arguments
    ///
    /// * `data` - 서명할 데이터
    ///
    /// # Returns
    ///
    /// CosmosSignature (r, s)
    pub fn sign_bytes(&self, data: &[u8]) -> CcxtResult<CosmosSignature> {
        sign_bytes(&self.keypair.private_key, data)
    }

    /// Amino 스타일 메시지 서명 (ADR-036)
    ///
    /// 오프체인 메시지 서명에 사용됩니다.
    ///
    /// # Arguments
    ///
    /// * `data` - Base64 인코딩된 데이터
    ///
    /// # Returns
    ///
    /// CosmosSignature
    pub fn sign_amino_message(&self, data: &str) -> CcxtResult<CosmosSignature> {
        sign_amino(
            &self.keypair.private_key,
            self.config.chain_id,
            &self.address,
            data,
        )
    }

    /// 서명 검증
    ///
    /// # Arguments
    ///
    /// * `data` - 원본 데이터
    /// * `signature` - 검증할 서명
    ///
    /// # Returns
    ///
    /// 검증 성공 여부
    pub fn verify(&self, data: &[u8], signature: &CosmosSignature) -> CcxtResult<bool> {
        verify_signature(&self.keypair.public_key, data, signature)
    }

    /// 다른 체인 주소 생성
    ///
    /// 동일한 공개키로 다른 체인의 주소를 생성합니다.
    ///
    /// # Arguments
    ///
    /// * `prefix` - 새 Bech32 접두사
    ///
    /// # Returns
    ///
    /// 새 체인의 주소
    pub fn address_for_chain(&self, prefix: &str) -> CcxtResult<String> {
        public_key_to_address(&self.keypair.public_key, prefix)
    }
}

impl std::fmt::Debug for CosmosWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CosmosWallet")
            .field("address", &self.address)
            .field("chain", &self.config.name)
            .field("public_key", &self.public_key_hex())
            .finish()
    }
}

/// dYdX v4 전용 지갑 확장
impl CosmosWallet {
    /// dYdX v4 메인넷 지갑 생성 (니모닉)
    pub fn dydx_mainnet(mnemonic: &str, index: u32) -> CcxtResult<Self> {
        Self::from_mnemonic(mnemonic, &super::address::DYDX_MAINNET, index)
    }

    /// dYdX v4 테스트넷 지갑 생성 (니모닉)
    pub fn dydx_testnet(mnemonic: &str, index: u32) -> CcxtResult<Self> {
        Self::from_mnemonic(mnemonic, &super::address::DYDX_TESTNET, index)
    }

    /// dYdX Subaccount ID 생성
    ///
    /// dYdX v4는 주소와 서브계정 번호로 서브계정을 식별합니다.
    ///
    /// # Arguments
    ///
    /// * `subaccount_number` - 서브계정 번호 (0-127)
    ///
    /// # Returns
    ///
    /// 서브계정 ID (주소/번호 형식)
    pub fn dydx_subaccount_id(&self, subaccount_number: u32) -> String {
        format!("{}/{}", self.address, subaccount_number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::cosmos::address::{DYDX_MAINNET, COSMOS_HUB};

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_wallet_from_mnemonic() {
        let wallet = CosmosWallet::from_mnemonic(
            TEST_MNEMONIC,
            &DYDX_MAINNET,
            0,
        ).unwrap();

        assert!(wallet.address().starts_with("dydx1"));
        assert_eq!(wallet.config().name, "dYdX v4 Mainnet");
    }

    #[test]
    fn test_wallet_different_indices() {
        let wallet0 = CosmosWallet::from_mnemonic(TEST_MNEMONIC, &DYDX_MAINNET, 0).unwrap();
        let wallet1 = CosmosWallet::from_mnemonic(TEST_MNEMONIC, &DYDX_MAINNET, 1).unwrap();

        assert_ne!(wallet0.address(), wallet1.address());
    }

    #[test]
    fn test_wallet_sign_and_verify() {
        let wallet = CosmosWallet::from_mnemonic(TEST_MNEMONIC, &COSMOS_HUB, 0).unwrap();
        let message = b"Test message for signing";

        let signature = wallet.sign_bytes(message).unwrap();
        let is_valid = wallet.verify(message, &signature).unwrap();

        assert!(is_valid);
    }

    #[test]
    fn test_wallet_address_for_chain() {
        let wallet = CosmosWallet::from_mnemonic(TEST_MNEMONIC, &DYDX_MAINNET, 0).unwrap();

        // dYdX 주소
        assert!(wallet.address().starts_with("dydx1"));

        // 동일 공개키로 Cosmos Hub 주소 생성
        let cosmos_addr = wallet.address_for_chain("cosmos").unwrap();
        assert!(cosmos_addr.starts_with("cosmos1"));

        // Osmosis 주소
        let osmo_addr = wallet.address_for_chain("osmo").unwrap();
        assert!(osmo_addr.starts_with("osmo1"));
    }

    #[test]
    fn test_dydx_shortcuts() {
        let mainnet = CosmosWallet::dydx_mainnet(TEST_MNEMONIC, 0).unwrap();
        let testnet = CosmosWallet::dydx_testnet(TEST_MNEMONIC, 0).unwrap();

        // 둘 다 dydx 접두사
        assert!(mainnet.address().starts_with("dydx1"));
        assert!(testnet.address().starts_with("dydx1"));

        // 동일한 니모닉 = 동일한 주소 (체인 ID만 다름)
        assert_eq!(mainnet.address(), testnet.address());

        // 체인 ID 확인
        assert_eq!(mainnet.config().chain_id, "dydx-mainnet-1");
        assert_eq!(testnet.config().chain_id, "dydx-testnet-4");
    }

    #[test]
    fn test_dydx_subaccount_id() {
        let wallet = CosmosWallet::dydx_mainnet(TEST_MNEMONIC, 0).unwrap();

        let sub0 = wallet.dydx_subaccount_id(0);
        let sub1 = wallet.dydx_subaccount_id(1);

        assert!(sub0.ends_with("/0"));
        assert!(sub1.ends_with("/1"));
        assert!(sub0.starts_with("dydx1"));
    }

    #[test]
    fn test_wallet_from_private_key_hex() {
        // 테스트용 개인키
        let hex_key = "0x0101010101010101010101010101010101010101010101010101010101010101";

        let wallet = CosmosWallet::from_private_key_hex(hex_key, &DYDX_MAINNET).unwrap();
        assert!(wallet.address().starts_with("dydx1"));

        // 0x 없이도 동작
        let hex_key2 = "0101010101010101010101010101010101010101010101010101010101010101";
        let wallet2 = CosmosWallet::from_private_key_hex(hex_key2, &DYDX_MAINNET).unwrap();

        assert_eq!(wallet.address(), wallet2.address());
    }

    #[test]
    fn test_wallet_debug() {
        let wallet = CosmosWallet::dydx_testnet(TEST_MNEMONIC, 0).unwrap();
        let debug_str = format!("{:?}", wallet);

        // 개인키는 출력되지 않아야 함
        assert!(!debug_str.contains("private"));
        // 주소와 공개키는 포함
        assert!(debug_str.contains("dydx1"));
        assert!(debug_str.contains("public_key"));
    }
}
