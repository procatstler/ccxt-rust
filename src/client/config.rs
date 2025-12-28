//! Exchange configuration

/// 거래소 설정
#[derive(Debug, Clone, Default)]
pub struct ExchangeConfig {
    api_key: Option<String>,
    api_secret: Option<String>,
    password: Option<String>,
    uid: Option<String>,
    token: Option<String>,
    sandbox: bool,
    timeout_ms: u64,
    rate_limit_ms: u64,
    hostname: Option<String>,
}

impl ExchangeConfig {
    /// 새로운 빈 설정 생성
    pub fn new() -> Self {
        Self {
            api_key: None,
            api_secret: None,
            password: None,
            uid: None,
            token: None,
            sandbox: false,
            timeout_ms: 30000,
            rate_limit_ms: 1000, // 초당 1회
            hostname: None,
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

    /// UID 설정 (일부 거래소 필요)
    pub fn with_uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    /// Token 설정 (JWT 등)
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
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

    /// 레이트 리밋 설정 (밀리초)
    pub fn with_rate_limit_ms(mut self, rate_limit_ms: u64) -> Self {
        self.rate_limit_ms = rate_limit_ms;
        self
    }

    /// 호스트네임 설정
    pub fn with_hostname(mut self, hostname: impl Into<String>) -> Self {
        self.hostname = Some(hostname.into());
        self
    }

    /// 인증 정보로 설정
    pub fn with_credentials(
        mut self,
        api_key: impl Into<String>,
        api_secret: impl Into<String>,
    ) -> Self {
        self.api_key = Some(api_key.into());
        self.api_secret = Some(api_secret.into());
        self
    }

    // === Getters ===

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn api_secret(&self) -> Option<&str> {
        self.api_secret.as_deref()
    }

    /// secret 별칭 (CCXT 호환)
    pub fn secret(&self) -> Option<&str> {
        self.api_secret.as_deref()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub fn uid(&self) -> Option<&str> {
        self.uid.as_deref()
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn is_sandbox(&self) -> bool {
        self.sandbox
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    pub fn rate_limit_ms(&self) -> u64 {
        self.rate_limit_ms
    }

    pub fn hostname(&self) -> Option<&str> {
        self.hostname.as_deref()
    }

    /// 인증 정보 유효성 확인
    pub fn has_credentials(&self) -> bool {
        self.api_key.is_some() && self.api_secret.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret")
            .with_timeout(5000);

        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
        assert_eq!(config.timeout_ms(), 5000);
        assert!(config.has_credentials());
    }

    #[test]
    fn test_config_default() {
        let config = ExchangeConfig::default();
        assert!(config.api_key().is_none());
        assert!(!config.has_credentials());
    }
}
