# DEX 지원 아키텍처 설계

## 1. 현재 상황 분석

### 1.1 지원 DEX 거래소

| 거래소 | 파일 | 인증 방식 | 체인 | 상태 |
|-------|------|----------|-----|------|
| Paradex | paradex.rs, paradex_ws.rs | StarkNet 서명 | StarkNet L2 | 프레임워크만 구현 |
| dYdX | dydx.rs, dydx_ws.rs | StarkNet 서명 | StarkNet L2 | 미구현 |
| Hyperliquid | hyperliquid.rs, hyperliquid_ws.rs | EIP-712 서명 | Arbitrum L2 | 미구현 |

### 1.2 공통 패턴 분석

#### CEX vs DEX 인증 차이

| 구분 | CEX (중앙화) | DEX (탈중앙화) |
|-----|-------------|---------------|
| 식별자 | API Key | Wallet Address |
| 비밀키 | API Secret | Private Key |
| 서명 방식 | HMAC-SHA256/512 | ECDSA (secp256k1) |
| 메시지 형식 | Query String / JSON | EIP-712 Typed Data |
| 인증 흐름 | 직접 서명 → 헤더 | 서명 → JWT 토큰 → 헤더 |

#### DEX 인증 플로우 (공통)

```
1. Wallet 연결
   └── Private Key → Public Key → Wallet Address

2. 메시지 서명
   └── EIP-712 Typed Data → Hash → ECDSA Sign

3. 인증 요청
   └── POST /auth (signature, address, timestamp)

4. JWT 토큰 획득
   └── Response: { jwt_token: "..." }

5. 인증된 요청
   └── Authorization: Bearer {jwt_token}
```

---

## 2. 문제점 및 개선 필요사항

### 2.1 현재 코드의 문제점

#### Paradex (paradex.rs:148-210)
```rust
// 현재: NotSupported 에러 반환
async fn authenticate(&self) -> CcxtResult<String> {
    // ... 프레임워크만 구현
    Err(CcxtError::NotSupported {
        feature: "Paradex authentication requires StarkNet signing...".into()
    })
}
```

#### Hyperliquid (hyperliquid.rs:1688-1695)
```rust
// 현재: NotSupported 에러 반환
async fn create_order(...) -> CcxtResult<Order> {
    Err(CcxtError::NotSupported {
        feature: "create_order requires EIP-712 wallet signing".into()
    })
}
```

### 2.2 필요한 암호화 기능

| 기능 | 용도 | 대상 DEX |
|-----|------|---------|
| Keccak256 | 메시지 해싱 | 모든 DEX |
| EIP-712 Encoding | 구조화된 데이터 인코딩 | Hyperliquid, Paradex |
| secp256k1 ECDSA | 서명 생성/검증 | 모든 EVM DEX |
| Poseidon Hash | StarkNet 해싱 | Paradex, dYdX |
| StarkNet Curve | StarkNet 서명 | Paradex, dYdX |
| Account Derivation | ETH → StarkNet 계정 | Paradex, dYdX |

---

## 3. 제안 아키텍처

### 3.1 디렉토리 구조

```
src/
├── crypto/                    # NEW: DEX 암호화 모듈
│   ├── mod.rs
│   ├── evm/                   # EVM 체인 공통
│   │   ├── mod.rs
│   │   ├── keccak.rs         # Keccak256 해싱
│   │   ├── eip712.rs         # EIP-712 타입 데이터
│   │   ├── secp256k1.rs      # ECDSA 서명
│   │   └── wallet.rs         # 지갑 관리
│   ├── starknet/              # StarkNet 체인 공통
│   │   ├── mod.rs
│   │   ├── poseidon.rs       # Poseidon 해싱
│   │   ├── curve.rs          # StarkNet 곡선
│   │   ├── typed_data.rs     # 구조화된 데이터
│   │   └── account.rs        # 계정 파생
│   └── common/                # 공통 유틸리티
│       ├── mod.rs
│       ├── traits.rs         # Signer, Hasher 트레이트
│       └── jwt.rs            # JWT 토큰 관리
│
├── exchanges/
│   ├── dex/                   # NEW: DEX 전용 모듈
│   │   ├── mod.rs
│   │   ├── traits.rs         # DexExchange 트레이트
│   │   ├── auth.rs           # DEX 인증 공통 로직
│   │   └── order_signing.rs  # 주문 서명 공통 로직
│   └── foreign/
│       ├── paradex.rs        # Paradex (crypto::starknet 사용)
│       ├── dydx.rs           # dYdX (crypto::starknet 사용)
│       └── hyperliquid.rs    # Hyperliquid (crypto::evm 사용)
│
└── utils/
    └── crypto.rs             # 기존 HMAC/JWT (CEX용)
```

### 3.2 핵심 트레이트 설계

#### 3.2.1 Signer 트레이트

```rust
// src/crypto/common/traits.rs

/// 서명자 트레이트 - DEX 서명의 공통 인터페이스
#[async_trait]
pub trait Signer: Send + Sync {
    /// 지갑 주소 반환
    fn address(&self) -> &str;

    /// 메시지 서명
    async fn sign_message(&self, message: &[u8]) -> CcxtResult<Vec<u8>>;

    /// 타입 데이터 서명 (EIP-712 스타일)
    async fn sign_typed_data(&self, typed_data: &TypedData) -> CcxtResult<Signature>;
}

/// EVM 호환 서명자
pub trait EvmSigner: Signer {
    /// Keccak256 해시
    fn keccak256(&self, data: &[u8]) -> [u8; 32];

    /// EIP-712 도메인 해시
    fn domain_separator(&self, domain: &Eip712Domain) -> [u8; 32];
}

/// StarkNet 서명자
pub trait StarkNetSigner: Signer {
    /// Poseidon 해시
    fn poseidon_hash(&self, data: &[FieldElement]) -> FieldElement;

    /// StarkNet 계정 파생
    fn derive_account(&self) -> CcxtResult<StarkNetAccount>;
}
```

#### 3.2.2 DexExchange 트레이트

```rust
// src/exchanges/dex/traits.rs

/// DEX 거래소 공통 트레이트
#[async_trait]
pub trait DexExchange: Exchange {
    /// 지갑 연결
    fn connect_wallet(&mut self, private_key: &str) -> CcxtResult<()>;

    /// 인증 (JWT 토큰 획득)
    async fn authenticate(&self) -> CcxtResult<AuthToken>;

    /// 서명된 주문 생성
    async fn create_signed_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<SignedOrder>;

    /// 서명된 주문 취소
    async fn cancel_signed_order(&self, order_id: &str) -> CcxtResult<()>;
}
```

### 3.3 EIP-712 구현

```rust
// src/crypto/evm/eip712.rs

/// EIP-712 도메인
#[derive(Debug, Clone, Serialize)]
pub struct Eip712Domain {
    pub name: String,
    pub version: String,
    pub chain_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifying_contract: Option<String>,
}

/// EIP-712 타입 정의
#[derive(Debug, Clone)]
pub struct TypedData {
    pub domain: Eip712Domain,
    pub primary_type: String,
    pub types: HashMap<String, Vec<TypedDataField>>,
    pub message: serde_json::Value,
}

/// 필드 타입
#[derive(Debug, Clone)]
pub struct TypedDataField {
    pub name: String,
    pub field_type: String,
}

impl TypedData {
    /// 구조화된 데이터 해시 (EIP-712)
    pub fn encode_hash(&self) -> CcxtResult<[u8; 32]> {
        let domain_separator = self.hash_domain()?;
        let struct_hash = self.hash_struct(&self.primary_type, &self.message)?;

        let mut data = Vec::new();
        data.push(0x19);
        data.push(0x01);
        data.extend_from_slice(&domain_separator);
        data.extend_from_slice(&struct_hash);

        Ok(keccak256(&data))
    }
}
```

### 3.4 StarkNet 구현

```rust
// src/crypto/starknet/account.rs

/// StarkNet 계정
#[derive(Debug, Clone)]
pub struct StarkNetAccount {
    pub address: String,
    pub public_key: String,
    pub private_key: String,
}

/// ETH 개인키에서 StarkNet 계정 파생
pub fn derive_starknet_account(
    eth_private_key: &str,
    account_hash: &str,
    proxy_hash: &str,
) -> CcxtResult<StarkNetAccount> {
    // 1. EIP-712 "STARK Key" 메시지 서명
    // 2. 서명 해시에서 StarkNet 키 쌍 파생
    // 3. 계정 주소 계산
    // ...
}

// src/crypto/starknet/typed_data.rs

/// StarkNet 구조화된 데이터 인코딩
pub fn encode_typed_data(
    domain: &StarkNetDomain,
    types: &HashMap<String, Vec<TypedDataField>>,
    message: &serde_json::Value,
    account_address: &str,
) -> CcxtResult<FieldElement> {
    // Poseidon 해시 기반 인코딩
    // ...
}
```

---

## 4. 구현 계획

### 4.1 Phase 1: 공통 인프라 (1-2주)

| 우선순위 | 작업 | 파일 |
|---------|------|------|
| 1 | crypto 모듈 구조 생성 | src/crypto/mod.rs |
| 2 | Keccak256 구현 | src/crypto/evm/keccak.rs |
| 3 | secp256k1 ECDSA 구현 | src/crypto/evm/secp256k1.rs |
| 4 | EIP-712 기본 구현 | src/crypto/evm/eip712.rs |
| 5 | 공통 트레이트 정의 | src/crypto/common/traits.rs |

**필요 크레이트**:
```toml
# Cargo.toml
[dependencies]
sha3 = "0.10"           # Keccak256
k256 = "0.13"           # secp256k1 ECDSA
ethers-core = "2.0"     # EIP-712 (선택적)
```

### 4.2 Phase 2: Hyperliquid 구현 (1주)

| 작업 | 설명 |
|-----|------|
| EVM Signer 구현 | 지갑 관리 및 서명 |
| Hyperliquid 인증 | JWT 토큰 획득 |
| 주문 서명 | create_order, cancel_order |
| 테스트 | 단위 테스트 + 통합 테스트 |

### 4.3 Phase 3: StarkNet 지원 (2주)

| 작업 | 설명 |
|-----|------|
| Poseidon 해시 | StarkNet 해싱 |
| StarkNet 곡선 | 서명 생성 |
| 계정 파생 | ETH → StarkNet |
| Paradex 구현 | 인증 + 주문 |
| dYdX 구현 | 인증 + 주문 |

**필요 크레이트**:
```toml
# Cargo.toml
[dependencies]
starknet = "0.9"        # StarkNet 코어
starknet-crypto = "0.6" # Poseidon, 곡선
```

### 4.4 Phase 4: 통합 및 최적화 (1주)

| 작업 | 설명 |
|-----|------|
| DexExchange 트레이트 | 공통 인터페이스 |
| 에러 처리 통합 | DEX 전용 에러 타입 |
| 문서화 | API 문서 + 예제 |
| 성능 최적화 | 서명 캐싱, 비동기 최적화 |

---

## 5. 외부 의존성

### 5.1 필수 크레이트

```toml
[dependencies]
# EVM 암호화
sha3 = "0.10"               # Keccak256
k256 = { version = "0.13", features = ["ecdsa"] }  # secp256k1

# StarkNet 암호화
starknet = "0.9"
starknet-crypto = "0.6"

# 선택적 (더 높은 수준 API)
ethers-core = "2.0"         # EIP-712
ethers-signers = "2.0"      # 지갑 관리
```

### 5.2 기존 의존성 활용

현재 `crypto.rs`에 이미 있는 기능:
- HMAC-SHA256/384/512 ✅
- Base64 인코딩 ✅
- JWT 생성/검증 ✅

---

## 6. 예상 코드 예시

### 6.1 Hyperliquid 주문 생성

```rust
// 개선 후 hyperliquid.rs
impl Hyperliquid {
    pub async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        // 1. 인증 확인
        let token = self.authenticate().await?;

        // 2. 주문 데이터 생성
        let order_data = self.build_order_data(symbol, order_type, side, amount, price)?;

        // 3. EIP-712 서명
        let typed_data = self.create_order_typed_data(&order_data)?;
        let signature = self.signer.sign_typed_data(&typed_data).await?;

        // 4. API 호출
        let request = OrderRequest {
            action: order_data,
            signature: signature.to_hex(),
            nonce: self.generate_nonce(),
        };

        self.private_post("/exchange", request).await
    }
}
```

### 6.2 Paradex 인증

```rust
// 개선 후 paradex.rs
impl Paradex {
    pub async fn authenticate(&self) -> CcxtResult<String> {
        // 캐시 확인
        if let Some(token) = self.get_cached_token() {
            return Ok(token);
        }

        // StarkNet 계정 파생
        let account = self.starknet_signer.derive_account()?;

        // 인증 요청 생성
        let now = Utc::now().timestamp();
        let expires = now + 180;
        let auth_request = AuthRequest {
            method: "POST",
            path: "/v1/auth",
            body: "",
            timestamp: now,
            expiration: expires,
        };

        // StarkNet 서명
        let typed_data = self.create_auth_typed_data(&auth_request, &account)?;
        let signature = self.starknet_signer.sign_typed_data(&typed_data).await?;

        // JWT 토큰 획득
        let response = self.post_auth(&account, &signature, now, expires).await?;

        // 캐시 저장
        self.cache_token(&response.jwt_token, expires);

        Ok(response.jwt_token)
    }
}
```

---

## 7. 결론

### 7.1 핵심 이점

1. **코드 재사용**: EVM/StarkNet 암호화 로직을 여러 DEX에서 공유
2. **유지보수성**: 암호화 로직 분리로 거래소 코드 단순화
3. **확장성**: 새로운 DEX 추가 시 기존 모듈 활용
4. **테스트 용이성**: 암호화 모듈 독립 테스트 가능

### 7.2 예상 작업량

| Phase | 작업량 | 기간 |
|-------|-------|-----|
| Phase 1: 공통 인프라 | 중 | 1-2주 |
| Phase 2: Hyperliquid | 중 | 1주 |
| Phase 3: StarkNet | 대 | 2주 |
| Phase 4: 통합 | 소 | 1주 |
| **총합** | - | **5-6주** |

### 7.3 다음 단계

1. 이 설계 문서 검토 및 승인
2. Phase 1 시작: crypto 모듈 구조 생성
3. 크레이트 의존성 추가 (Cargo.toml)
4. 단계별 구현 및 테스트
