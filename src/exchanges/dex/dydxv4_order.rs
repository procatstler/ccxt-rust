//! dYdX v4 Order Types and Transaction Building
//!
//! Cosmos SDK 기반 dYdX v4 주문 생성 및 트랜잭션 빌딩
//!
//! # Architecture
//!
//! - Short-term orders: 20블록 이내 만료, 낮은 수수료
//! - Long-term orders: 블록 시간 기준 만료
//! - Conditional orders: 조건부 주문 (스탑, 테이크프로핏)
//!
//! # References
//!
//! - [dYdX v4 Protocol](https://github.com/dydxprotocol/v4-chain)
//! - [Order Types](https://docs.dydx.exchange/concepts-trading/types_of_orders)

#![allow(dead_code)]

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::crypto::cosmos::protobuf::*;
use crate::crypto::cosmos::transaction::{
    CosmosAny, CosmosAuthInfo, CosmosFee, CosmosSignDoc, CosmosSignerInfo, CosmosTxBody,
    CosmosTxRaw,
};
use crate::crypto::cosmos::CosmosWallet;
use crate::errors::CcxtResult;

// ============================================================================
// Constants
// ============================================================================

/// Short-term order window (blocks)
pub const SHORT_BLOCK_WINDOW: u32 = 20;

/// Order flags
pub mod order_flags {
    /// Short-term order (expires within SHORT_BLOCK_WINDOW blocks)
    pub const SHORT_TERM: u32 = 0;
    /// Long-term order (expires at specific block time)
    pub const LONG_TERM: u32 = 64;
    /// Conditional order (stop-loss, take-profit)
    pub const CONDITIONAL: u32 = 32;
}

// ============================================================================
// Protobuf Message Types (dYdX v4 specific)
// ============================================================================

/// SubaccountId - dYdX 서브어카운트 식별자
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubaccountId {
    /// 지갑 주소 (dydx1...)
    pub owner: String,
    /// 서브어카운트 번호 (0-127)
    pub number: u32,
}

impl SubaccountId {
    pub fn new(owner: &str, number: u32) -> Self {
        Self {
            owner: owner.to_string(),
            number,
        }
    }

    /// Protobuf 인코딩
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: owner (string)
        encode_string(&mut buf, 1, &self.owner);
        // Field 2: number (uint32)
        encode_uint32(&mut buf, 2, self.number);
        buf
    }
}

/// OrderId - 주문 고유 식별자
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderId {
    /// 서브어카운트
    pub subaccount_id: SubaccountId,
    /// 클라이언트 ID (fixed32)
    pub client_id: u32,
    /// 주문 플래그
    pub order_flags: u32,
    /// CLOB 페어 ID
    pub clob_pair_id: u32,
}

impl OrderId {
    pub fn new(
        subaccount_id: SubaccountId,
        client_id: u32,
        order_flags: u32,
        clob_pair_id: u32,
    ) -> Self {
        Self {
            subaccount_id,
            client_id,
            order_flags,
            clob_pair_id,
        }
    }

    /// Protobuf 인코딩
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: subaccount_id (message)
        let subaccount_bytes = self.subaccount_id.encode();
        encode_length_delimited(&mut buf, 1, &subaccount_bytes);
        // Field 2: client_id (fixed32)
        encode_fixed32(&mut buf, 2, self.client_id);
        // Field 3: order_flags (uint32)
        encode_uint32(&mut buf, 3, self.order_flags);
        // Field 4: clob_pair_id (uint32)
        encode_uint32(&mut buf, 4, self.clob_pair_id);
        buf
    }
}

/// Order Side
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum DydxOrderSide {
    Unspecified = 0,
    Buy = 1,
    Sell = 2,
}

impl From<crate::types::OrderSide> for DydxOrderSide {
    fn from(side: crate::types::OrderSide) -> Self {
        match side {
            crate::types::OrderSide::Buy => DydxOrderSide::Buy,
            crate::types::OrderSide::Sell => DydxOrderSide::Sell,
        }
    }
}

/// Time in Force
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum DydxTimeInForce {
    /// Default behavior (GTC for long-term, IOC for market)
    Unspecified = 0,
    /// Immediate or Cancel
    Ioc = 1,
    /// Post Only (maker only)
    PostOnly = 2,
    /// Fill or Kill
    FillOrKill = 3,
}

impl From<crate::types::TimeInForce> for DydxTimeInForce {
    fn from(tif: crate::types::TimeInForce) -> Self {
        match tif {
            crate::types::TimeInForce::GTC => DydxTimeInForce::Unspecified,
            crate::types::TimeInForce::IOC => DydxTimeInForce::Ioc,
            crate::types::TimeInForce::FOK => DydxTimeInForce::FillOrKill,
            crate::types::TimeInForce::PO => DydxTimeInForce::PostOnly,
            _ => DydxTimeInForce::Unspecified,
        }
    }
}

/// Condition Type (for conditional orders)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ConditionType {
    Unspecified = 0,
    StopLoss = 1,
    TakeProfit = 2,
}

/// Order - dYdX v4 주문
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DydxOrder {
    /// 주문 ID
    pub order_id: OrderId,
    /// 주문 방향
    pub side: DydxOrderSide,
    /// 수량 (base quantums)
    pub quantums: u64,
    /// 가격 (subticks)
    pub subticks: u64,
    /// 만료 조건 (GoodTilBlock or GoodTilBlockTime)
    pub good_til_oneof: GoodTilOneof,
    /// 주문 유형
    pub time_in_force: DydxTimeInForce,
    /// 포지션 축소 전용
    pub reduce_only: bool,
    /// 클라이언트 메타데이터
    pub client_metadata: u32,
    /// 조건 유형 (조건부 주문용)
    pub condition_type: ConditionType,
    /// 조건부 주문 트리거 가격
    pub conditional_order_trigger_subticks: u64,
}

/// GoodTilOneof - 주문 만료 조건
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GoodTilOneof {
    /// 블록 높이 기준 만료 (short-term orders)
    GoodTilBlock(u32),
    /// 블록 시간 기준 만료 (long-term orders)
    GoodTilBlockTime(u32),
}

// ============================================================================
// Builder Pattern - DydxOrderParams
// ============================================================================

/// Short-term 주문 생성 파라미터 (Builder 패턴)
///
/// 10개 이상의 파라미터를 구조화하여 Clippy too_many_arguments 경고 해결
#[derive(Clone, Debug)]
pub struct ShortTermOrderParams<'a> {
    /// 지갑 소유자 주소
    pub owner: &'a str,
    /// 서브어카운트 번호
    pub subaccount_number: u32,
    /// 클라이언트 ID
    pub client_id: u32,
    /// CLOB 페어 ID
    pub clob_pair_id: u32,
    /// 주문 방향
    pub side: DydxOrderSide,
    /// 수량 (quantums)
    pub quantums: u64,
    /// 가격 (subticks)
    pub subticks: u64,
    /// 만료 블록
    pub good_til_block: u32,
    /// TimeInForce
    pub time_in_force: DydxTimeInForce,
    /// 포지션 축소 전용 여부
    pub reduce_only: bool,
}

/// Long-term 주문 생성 파라미터 (Builder 패턴)
#[derive(Clone, Debug)]
pub struct LongTermOrderParams<'a> {
    /// 지갑 소유자 주소
    pub owner: &'a str,
    /// 서브어카운트 번호
    pub subaccount_number: u32,
    /// 클라이언트 ID
    pub client_id: u32,
    /// CLOB 페어 ID
    pub clob_pair_id: u32,
    /// 주문 방향
    pub side: DydxOrderSide,
    /// 수량 (quantums)
    pub quantums: u64,
    /// 가격 (subticks)
    pub subticks: u64,
    /// 만료 시간 (Unix timestamp)
    pub good_til_block_time: u32,
    /// TimeInForce
    pub time_in_force: DydxTimeInForce,
    /// 포지션 축소 전용 여부
    pub reduce_only: bool,
}

impl DydxOrder {
    /// 새 short-term 주문 생성 (Builder 패턴)
    pub fn from_short_term_params(params: ShortTermOrderParams<'_>) -> Self {
        Self {
            order_id: OrderId::new(
                SubaccountId::new(params.owner, params.subaccount_number),
                params.client_id,
                order_flags::SHORT_TERM,
                params.clob_pair_id,
            ),
            side: params.side,
            quantums: params.quantums,
            subticks: params.subticks,
            good_til_oneof: GoodTilOneof::GoodTilBlock(params.good_til_block),
            time_in_force: params.time_in_force,
            reduce_only: params.reduce_only,
            client_metadata: 0,
            condition_type: ConditionType::Unspecified,
            conditional_order_trigger_subticks: 0,
        }
    }

    /// 새 short-term 주문 생성 (기존 API 호환, deprecated)
    #[allow(clippy::too_many_arguments)]
    #[deprecated(since = "0.1.0", note = "Use from_short_term_params instead")]
    pub fn new_short_term(
        owner: &str,
        subaccount_number: u32,
        client_id: u32,
        clob_pair_id: u32,
        side: DydxOrderSide,
        quantums: u64,
        subticks: u64,
        good_til_block: u32,
        time_in_force: DydxTimeInForce,
        reduce_only: bool,
    ) -> Self {
        Self::from_short_term_params(ShortTermOrderParams {
            owner,
            subaccount_number,
            client_id,
            clob_pair_id,
            side,
            quantums,
            subticks,
            good_til_block,
            time_in_force,
            reduce_only,
        })
    }

    /// 새 long-term 주문 생성 (Builder 패턴)
    pub fn from_long_term_params(params: LongTermOrderParams<'_>) -> Self {
        Self {
            order_id: OrderId::new(
                SubaccountId::new(params.owner, params.subaccount_number),
                params.client_id,
                order_flags::LONG_TERM,
                params.clob_pair_id,
            ),
            side: params.side,
            quantums: params.quantums,
            subticks: params.subticks,
            good_til_oneof: GoodTilOneof::GoodTilBlockTime(params.good_til_block_time),
            time_in_force: params.time_in_force,
            reduce_only: params.reduce_only,
            client_metadata: 0,
            condition_type: ConditionType::Unspecified,
            conditional_order_trigger_subticks: 0,
        }
    }

    /// 새 long-term 주문 생성 (기존 API 호환, deprecated)
    #[allow(clippy::too_many_arguments)]
    #[deprecated(since = "0.1.0", note = "Use from_long_term_params instead")]
    pub fn new_long_term(
        owner: &str,
        subaccount_number: u32,
        client_id: u32,
        clob_pair_id: u32,
        side: DydxOrderSide,
        quantums: u64,
        subticks: u64,
        good_til_block_time: u32,
        time_in_force: DydxTimeInForce,
        reduce_only: bool,
    ) -> Self {
        Self::from_long_term_params(LongTermOrderParams {
            owner,
            subaccount_number,
            client_id,
            clob_pair_id,
            side,
            quantums,
            subticks,
            good_til_block_time,
            time_in_force,
            reduce_only,
        })
    }

    /// Protobuf 인코딩
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Field 1: order_id (message)
        let order_id_bytes = self.order_id.encode();
        encode_length_delimited(&mut buf, 1, &order_id_bytes);

        // Field 2: side (enum as uint32)
        encode_uint32(&mut buf, 2, self.side as u32);

        // Field 3: quantums (uint64)
        encode_uint64(&mut buf, 3, self.quantums);

        // Field 4: subticks (uint64)
        encode_uint64(&mut buf, 4, self.subticks);

        // Field 5 or 6: good_til_oneof
        match &self.good_til_oneof {
            GoodTilOneof::GoodTilBlock(block) => {
                encode_uint32(&mut buf, 5, *block);
            },
            GoodTilOneof::GoodTilBlockTime(time) => {
                encode_fixed32(&mut buf, 6, *time);
            },
        }

        // Field 7: time_in_force (enum as uint32)
        encode_uint32(&mut buf, 7, self.time_in_force as u32);

        // Field 8: reduce_only (bool)
        if self.reduce_only {
            encode_bool(&mut buf, 8, true);
        }

        // Field 9: client_metadata (uint32)
        if self.client_metadata != 0 {
            encode_uint32(&mut buf, 9, self.client_metadata);
        }

        // Field 10: condition_type (enum as uint32)
        if self.condition_type != ConditionType::Unspecified {
            encode_uint32(&mut buf, 10, self.condition_type as u32);
        }

        // Field 11: conditional_order_trigger_subticks (uint64)
        if self.conditional_order_trigger_subticks != 0 {
            encode_uint64(&mut buf, 11, self.conditional_order_trigger_subticks);
        }

        buf
    }
}

/// MsgPlaceOrder - 주문 생성 메시지
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MsgPlaceOrder {
    pub order: DydxOrder,
}

impl MsgPlaceOrder {
    pub const TYPE_URL: &'static str = "/dydxprotocol.clob.MsgPlaceOrder";

    pub fn new(order: DydxOrder) -> Self {
        Self { order }
    }

    /// Protobuf 인코딩
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: order (message)
        let order_bytes = self.order.encode();
        encode_length_delimited(&mut buf, 1, &order_bytes);
        buf
    }

    /// Any 타입으로 래핑
    pub fn to_any(&self) -> CosmosAny {
        CosmosAny {
            type_url: Self::TYPE_URL.to_string(),
            value: self.encode(),
        }
    }
}

/// MsgCancelOrder - 주문 취소 메시지
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MsgCancelOrder {
    pub order_id: OrderId,
    pub good_til_oneof: GoodTilOneof,
}

impl MsgCancelOrder {
    pub const TYPE_URL: &'static str = "/dydxprotocol.clob.MsgCancelOrder";

    pub fn new(order_id: OrderId, good_til_oneof: GoodTilOneof) -> Self {
        Self {
            order_id,
            good_til_oneof,
        }
    }

    /// Protobuf 인코딩
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // Field 1: order_id (message)
        let order_id_bytes = self.order_id.encode();
        encode_length_delimited(&mut buf, 1, &order_id_bytes);

        // Field 2 or 3: good_til_oneof
        match &self.good_til_oneof {
            GoodTilOneof::GoodTilBlock(block) => {
                encode_uint32(&mut buf, 2, *block);
            },
            GoodTilOneof::GoodTilBlockTime(time) => {
                encode_fixed32(&mut buf, 3, *time);
            },
        }

        buf
    }

    /// Any 타입으로 래핑
    pub fn to_any(&self) -> CosmosAny {
        CosmosAny {
            type_url: Self::TYPE_URL.to_string(),
            value: self.encode(),
        }
    }
}

// ============================================================================
// Transaction Builder
// ============================================================================

/// dYdX v4 트랜잭션 빌더
pub struct DydxTransactionBuilder {
    chain_id: String,
}

impl DydxTransactionBuilder {
    /// 테스트넷 여부로 트랜잭션 빌더 생성
    pub fn new(testnet: bool) -> Self {
        if testnet {
            Self::testnet()
        } else {
            Self::mainnet()
        }
    }

    /// 메인넷 트랜잭션 빌더
    pub fn mainnet() -> Self {
        Self {
            chain_id: "dydx-mainnet-1".to_string(),
        }
    }

    /// 테스트넷 트랜잭션 빌더
    pub fn testnet() -> Self {
        Self {
            chain_id: "dydx-testnet-4".to_string(),
        }
    }

    /// 체인 ID 반환
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }

    /// MsgPlaceOrder 트랜잭션 빌드 및 서명
    pub fn build_place_order_tx(
        &self,
        wallet: &CosmosWallet,
        order: DydxOrder,
        account_number: u64,
        sequence: u64,
        memo: &str,
    ) -> CcxtResult<Vec<u8>> {
        let msg = MsgPlaceOrder::new(order);
        self.build_and_sign_tx(wallet, vec![msg.to_any()], account_number, sequence, memo)
    }

    /// MsgCancelOrder 트랜잭션 빌드 및 서명
    pub fn build_cancel_order_tx(
        &self,
        wallet: &CosmosWallet,
        order_id: OrderId,
        good_til_oneof: GoodTilOneof,
        account_number: u64,
        sequence: u64,
        memo: &str,
    ) -> CcxtResult<Vec<u8>> {
        let msg = MsgCancelOrder::new(order_id, good_til_oneof);
        self.build_and_sign_tx(wallet, vec![msg.to_any()], account_number, sequence, memo)
    }

    /// 트랜잭션 빌드 및 서명
    fn build_and_sign_tx(
        &self,
        wallet: &CosmosWallet,
        messages: Vec<CosmosAny>,
        account_number: u64,
        sequence: u64,
        memo: &str,
    ) -> CcxtResult<Vec<u8>> {
        // Build TxBody
        let tx_body = CosmosTxBody::new(messages, memo);
        let body_bytes = tx_body.encode();

        // Build AuthInfo (zero fee for dYdX v4)
        let signer_info = CosmosSignerInfo::new(wallet.public_key(), sequence);
        let auth_info = CosmosAuthInfo::new(vec![signer_info], CosmosFee::zero());
        let auth_info_bytes = auth_info.encode();

        // Build SignDoc
        let sign_doc = CosmosSignDoc {
            body_bytes: body_bytes.clone(),
            auth_info_bytes: auth_info_bytes.clone(),
            chain_id: self.chain_id.clone(),
            account_number,
        };

        // Sign
        let sign_doc_bytes = sign_doc.encode();
        let signature = wallet.sign_bytes(&sign_doc_bytes)?;

        // Build TxRaw
        let tx_raw = CosmosTxRaw {
            body_bytes,
            auth_info_bytes,
            signatures: vec![signature.to_bytes().to_vec()],
        };

        Ok(tx_raw.encode())
    }
}

// ============================================================================
// Market Data Helpers
// ============================================================================

/// 마켓 데이터 (quantums/subticks 변환용)
#[derive(Clone, Debug)]
pub struct DydxMarketInfo {
    /// CLOB 페어 ID
    pub clob_pair_id: u32,
    /// 원자 해상도 (예: -10 = 10^-10)
    pub atomic_resolution: i32,
    /// 스텝 기본 quantums
    pub step_base_quantums: u64,
    /// subticks per tick
    pub subticks_per_tick: u32,
    /// quantum conversion exponent
    pub quantum_conversion_exponent: i32,
}

impl DydxMarketInfo {
    /// 크기를 quantums로 변환
    pub fn size_to_quantums(&self, size: Decimal) -> u64 {
        // quantums = size * 10^atomic_resolution / step_base_quantums
        let multiplier = Decimal::from(10i64.pow((-self.atomic_resolution) as u32));
        let raw_quantums = size * multiplier;
        let quantums = raw_quantums / Decimal::from(self.step_base_quantums);
        // Round to nearest step

        (quantums.round() * Decimal::from(self.step_base_quantums))
            .to_string()
            .parse::<u64>()
            .unwrap_or(0)
    }

    /// 가격을 subticks로 변환
    ///
    /// dYdX v4 공식: subticks = price * 10^(-quantumConversionExponent) * subticksPerTick
    pub fn price_to_subticks(&self, price: Decimal) -> u64 {
        // subticks = price * 10^(-quantum_conversion_exponent) * subticks_per_tick
        let exponent = -self.quantum_conversion_exponent;
        let multiplier = if exponent >= 0 {
            Decimal::from(10i64.pow(exponent as u32))
        } else {
            Decimal::ONE / Decimal::from(10i64.pow((-exponent) as u32))
        };
        let raw_subticks = price * multiplier * Decimal::from(self.subticks_per_tick);
        // Round to nearest subticks_per_tick
        let ticks = (raw_subticks / Decimal::from(self.subticks_per_tick)).round();

        (ticks * Decimal::from(self.subticks_per_tick))
            .to_string()
            .parse::<u64>()
            .unwrap_or(0)
    }
}

// ============================================================================
// Node API Client
// ============================================================================

use serde::de::DeserializeOwned;

/// dYdX v4 Node API 응답
#[derive(Clone, Debug, Deserialize)]
pub struct NodeApiResponse<T> {
    pub result: Option<T>,
    pub error: Option<NodeApiError>,
}

/// Node API 에러
#[derive(Clone, Debug, Deserialize)]
pub struct NodeApiError {
    pub code: i32,
    pub message: String,
    pub data: Option<String>,
}

/// 브로드캐스트 결과
#[derive(Clone, Debug, Deserialize)]
pub struct BroadcastTxResult {
    pub code: u32,
    pub data: Option<String>,
    pub log: Option<String>,
    pub codespace: Option<String>,
    pub hash: String,
}

/// 동기 브로드캐스트 결과
#[derive(Clone, Debug, Deserialize)]
pub struct BroadcastTxSyncResult {
    pub code: u32,
    pub data: Option<String>,
    pub log: Option<String>,
    pub hash: String,
}

/// 계정 정보
#[derive(Clone, Debug, Deserialize)]
pub struct AccountInfo {
    #[serde(rename = "account")]
    pub account: AccountDetails,
}

/// 계정 상세 정보
#[derive(Clone, Debug, Deserialize)]
pub struct AccountDetails {
    #[serde(rename = "@type")]
    pub type_url: Option<String>,
    pub address: String,
    pub pub_key: Option<AccountPubKey>,
    pub account_number: String,
    pub sequence: String,
}

/// 계정 공개키
#[derive(Clone, Debug, Deserialize)]
pub struct AccountPubKey {
    #[serde(rename = "@type")]
    pub type_url: String,
    pub key: String,
}

/// 블록 높이 정보
#[derive(Clone, Debug, Deserialize)]
pub struct BlockHeight {
    pub block: BlockInfo,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlockInfo {
    pub header: BlockHeader,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlockHeader {
    pub height: String,
}

/// dYdX v4 Node API 클라이언트
pub struct DydxNodeClient {
    /// Node RPC URL
    rpc_url: String,
    /// REST API URL
    rest_url: String,
    /// HTTP 클라이언트
    http: reqwest::Client,
}

impl DydxNodeClient {
    /// Mainnet 클라이언트
    pub fn mainnet() -> Self {
        Self {
            rpc_url: "https://dydx-ops-rpc.kingnodes.com".to_string(),
            rest_url: "https://dydx-ops-rest.kingnodes.com".to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Testnet 클라이언트
    pub fn testnet() -> Self {
        Self {
            rpc_url: "https://dydx-testnet-rpc.polkachu.com".to_string(),
            rest_url: "https://dydx-testnet-api.polkachu.com".to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// 테스트넷 여부로 클라이언트 생성
    pub fn new(testnet: bool) -> Self {
        if testnet {
            Self::testnet()
        } else {
            Self::mainnet()
        }
    }

    /// 커스텀 URL로 클라이언트 생성
    pub fn with_urls(rpc_url: &str, rest_url: &str) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
            rest_url: rest_url.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// 계정 정보 조회
    pub async fn get_account(&self, address: &str) -> CcxtResult<AccountDetails> {
        let url = format!("{}/cosmos/auth/v1beta1/accounts/{}", self.rest_url, address);
        let resp: AccountInfo = self.http_get(&url).await?;
        Ok(resp.account)
    }

    /// 현재 블록 높이 조회
    pub async fn get_latest_block_height(&self) -> CcxtResult<u32> {
        let url = format!(
            "{}/cosmos/base/tendermint/v1beta1/blocks/latest",
            self.rest_url
        );
        let resp: BlockHeight = self.http_get(&url).await?;
        resp.block
            .header
            .height
            .parse()
            .map_err(|_| crate::errors::CcxtError::ExchangeError {
                message: "Failed to parse block height".to_string(),
            })
    }

    /// 트랜잭션 비동기 브로드캐스트 (빠름, 결과 확인 안함)
    pub async fn broadcast_tx_async(&self, tx_bytes: &[u8]) -> CcxtResult<String> {
        let tx_base64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, tx_bytes);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "broadcast_tx_async",
            "params": {
                "tx": tx_base64
            }
        });

        let resp: serde_json::Value = self
            .http
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::errors::CcxtError::NetworkError {
                url: self.rpc_url.clone(),
                message: e.to_string(),
            })?
            .json()
            .await
            .map_err(|e| crate::errors::CcxtError::ExchangeError {
                message: e.to_string(),
            })?;

        // Check for error
        if let Some(error) = resp.get("error") {
            return Err(crate::errors::CcxtError::ExchangeError {
                message: error.to_string(),
            });
        }

        // Extract hash
        let hash = resp["result"]["hash"].as_str().unwrap_or("").to_string();

        Ok(hash)
    }

    /// 트랜잭션 동기 브로드캐스트 (CheckTx 대기)
    pub async fn broadcast_tx_sync(&self, tx_bytes: &[u8]) -> CcxtResult<BroadcastTxSyncResult> {
        let tx_base64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, tx_bytes);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "broadcast_tx_sync",
            "params": {
                "tx": tx_base64
            }
        });

        let resp: serde_json::Value = self
            .http
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::errors::CcxtError::NetworkError {
                url: self.rpc_url.clone(),
                message: e.to_string(),
            })?
            .json()
            .await
            .map_err(|e| crate::errors::CcxtError::ExchangeError {
                message: e.to_string(),
            })?;

        // Check for RPC error
        if let Some(error) = resp.get("error") {
            return Err(crate::errors::CcxtError::ExchangeError {
                message: error.to_string(),
            });
        }

        let result = &resp["result"];

        // Check transaction error
        let code = result["code"].as_u64().unwrap_or(0) as u32;
        if code != 0 {
            let log = result["log"].as_str().unwrap_or("Unknown error");
            return Err(crate::errors::CcxtError::ExchangeError {
                message: format!("Transaction failed: code={code}, log={log}"),
            });
        }

        Ok(BroadcastTxSyncResult {
            code,
            data: result["data"].as_str().map(|s| s.to_string()),
            log: result["log"].as_str().map(|s| s.to_string()),
            hash: result["hash"].as_str().unwrap_or("").to_string(),
        })
    }

    /// HTTP GET 요청
    async fn http_get<T: DeserializeOwned>(&self, url: &str) -> CcxtResult<T> {
        self.http
            .get(url)
            .send()
            .await
            .map_err(|e| crate::errors::CcxtError::NetworkError {
                url: self.rpc_url.clone(),
                message: e.to_string(),
            })?
            .json()
            .await
            .map_err(|e| crate::errors::CcxtError::ExchangeError {
                message: e.to_string(),
            })
    }
}

/// 클라이언트 ID 생성 (타임스탬프 기반)
pub fn generate_client_id() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u32;
    now % 0x7FFFFFFF // 양수 범위 유지
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subaccount_id_encode() {
        let subaccount = SubaccountId::new("dydx1abc123", 0);
        let encoded = subaccount.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_order_id_encode() {
        let order_id = OrderId::new(
            SubaccountId::new("dydx1abc123", 0),
            12345,
            order_flags::SHORT_TERM,
            0,
        );
        let encoded = order_id.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_short_term_order_encode() {
        let order = DydxOrder::from_short_term_params(ShortTermOrderParams {
            owner: "dydx1abc123",
            subaccount_number: 0,
            client_id: 12345,
            clob_pair_id: 0, // BTC-USD
            side: DydxOrderSide::Buy,
            quantums: 1000000000,
            subticks: 5000000000,
            good_til_block: 100,
            time_in_force: DydxTimeInForce::Unspecified,
            reduce_only: false,
        });
        let encoded = order.encode();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_msg_place_order_encode() {
        let order = DydxOrder::from_short_term_params(ShortTermOrderParams {
            owner: "dydx1abc123",
            subaccount_number: 0,
            client_id: 12345,
            clob_pair_id: 0,
            side: DydxOrderSide::Buy,
            quantums: 1000000000,
            subticks: 5000000000,
            good_til_block: 100,
            time_in_force: DydxTimeInForce::Unspecified,
            reduce_only: false,
        });
        let msg = MsgPlaceOrder::new(order);
        let encoded = msg.encode();
        assert!(!encoded.is_empty());
        assert_eq!(msg.to_any().type_url, "/dydxprotocol.clob.MsgPlaceOrder");
    }

    #[test]
    fn test_msg_cancel_order_encode() {
        let order_id = OrderId::new(
            SubaccountId::new("dydx1abc123", 0),
            12345,
            order_flags::SHORT_TERM,
            0,
        );
        let msg = MsgCancelOrder::new(order_id, GoodTilOneof::GoodTilBlock(100));
        let encoded = msg.encode();
        assert!(!encoded.is_empty());
        assert_eq!(msg.to_any().type_url, "/dydxprotocol.clob.MsgCancelOrder");
    }

    #[test]
    fn test_market_info_conversions() {
        // BTC-USD market example
        let market = DydxMarketInfo {
            clob_pair_id: 0,
            atomic_resolution: -10,
            step_base_quantums: 1000000,
            subticks_per_tick: 100000,
            quantum_conversion_exponent: -9,
        };

        // 0.001 BTC
        let size = Decimal::from_str_exact("0.001").unwrap();
        let quantums = market.size_to_quantums(size);
        assert!(quantums > 0);

        // $50,000 price
        let price = Decimal::from_str_exact("50000").unwrap();
        let subticks = market.price_to_subticks(price);
        assert!(subticks > 0);
    }

    #[test]
    fn test_cosmos_tx_body_encode() {
        let order = DydxOrder::from_short_term_params(ShortTermOrderParams {
            owner: "dydx1abc123",
            subaccount_number: 0,
            client_id: 1,
            clob_pair_id: 0,
            side: DydxOrderSide::Buy,
            quantums: 1000000000,
            subticks: 5000000000,
            good_til_block: 100,
            time_in_force: DydxTimeInForce::Unspecified,
            reduce_only: false,
        });
        let msg = MsgPlaceOrder::new(order);
        let tx_body = CosmosTxBody::new(vec![msg.to_any()], "");
        let encoded = tx_body.encode();
        assert!(!encoded.is_empty());
    }
}
