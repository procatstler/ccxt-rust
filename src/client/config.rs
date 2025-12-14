//! Exchange configuration

use shared_types::common::Exchange as ExchangeId;

/// 거래소 설정
#[derive(Debug, Clone)]
pub struct ExchangeConfig {
    exchange_id: ExchangeId,
    api_key: Option<String>,
    api_secret: Option<String>,
    password: Option<String>,
    sandbox: bool,
    timeout_ms: u64,
    rate_limit: u32,
}

impl ExchangeConfig {
    /// 새로운 설정 생성
    pub fn new(exchange_id: ExchangeId) -> Self {
        Self {
            exchange_id,
            api_key: None,
            api_secret: None,
            password: None,
            sandbox: false,
            timeout_ms: 30000,
            rate_limit: 10,
        }
    }

    /// API 키 설정
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// API 시크릿 설정
    pub fn with_api_secret(mut self, secret: impl Into<String>) -> Self {
        self.api_secret = Some(secret.into());
        self
    }

    /// 비밀번호 설정 (일부 거래소 필요)
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// 샌드박스 모드 설정
    pub fn with_sandbox(mut self, sandbox: bool) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// 타임아웃 설정 (밀리초)
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// 레이트 리밋 설정 (초당 요청 수)
    pub fn with_rate_limit(mut self, rate_limit: u32) -> Self {
        self.rate_limit = rate_limit;
        self
    }

    // Getters

    pub fn exchange_id(&self) -> ExchangeId {
        self.exchange_id
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn api_secret(&self) -> Option<&str> {
        self.api_secret.as_deref()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub fn is_sandbox(&self) -> bool {
        self.sandbox
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    pub fn rate_limit(&self) -> u32 {
        self.rate_limit
    }
}
