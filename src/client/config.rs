//! Exchange configuration

/// 프록시 설정
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    /// HTTP/HTTPS 프록시 URL (예: "http://127.0.0.1:8080")
    pub http_proxy: Option<String>,
    /// SOCKS5 프록시 URL (예: "socks5://127.0.0.1:1080")
    pub socks_proxy: Option<String>,
    /// 프록시 인증 사용자명
    pub proxy_username: Option<String>,
    /// 프록시 인증 비밀번호
    pub proxy_password: Option<String>,
    /// 프록시 바이패스 호스트 목록
    pub no_proxy: Vec<String>,
}

impl ProxyConfig {
    /// 새로운 프록시 설정 생성
    pub fn new() -> Self {
        Self::default()
    }

    /// HTTP 프록시 설정
    pub fn with_http_proxy(mut self, proxy: impl Into<String>) -> Self {
        self.http_proxy = Some(proxy.into());
        self
    }

    /// SOCKS5 프록시 설정
    pub fn with_socks_proxy(mut self, proxy: impl Into<String>) -> Self {
        self.socks_proxy = Some(proxy.into());
        self
    }

    /// 프록시 인증 설정
    pub fn with_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.proxy_username = Some(username.into());
        self.proxy_password = Some(password.into());
        self
    }

    /// 바이패스 호스트 추가
    pub fn with_no_proxy(mut self, hosts: Vec<String>) -> Self {
        self.no_proxy = hosts;
        self
    }

    /// 프록시 설정 여부 확인
    pub fn is_configured(&self) -> bool {
        self.http_proxy.is_some() || self.socks_proxy.is_some()
    }
}

/// 재시도 설정
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// 최대 재시도 횟수
    pub max_retries: u32,
    /// 초기 대기 시간 (밀리초)
    pub initial_delay_ms: u64,
    /// 최대 대기 시간 (밀리초)
    pub max_delay_ms: u64,
    /// 지수 백오프 배수
    pub backoff_multiplier: f64,
    /// 재시도할 HTTP 상태 코드
    pub retry_status_codes: Vec<u16>,
    /// 네트워크 에러 재시도 여부
    pub retry_on_network_error: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            retry_status_codes: vec![429, 500, 502, 503, 504],
            retry_on_network_error: true,
        }
    }
}

impl RetryConfig {
    /// 새로운 재시도 설정 생성
    pub fn new() -> Self {
        Self::default()
    }

    /// 최대 재시도 횟수 설정
    pub fn with_max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    /// 초기 대기 시간 설정
    pub fn with_initial_delay(mut self, delay_ms: u64) -> Self {
        self.initial_delay_ms = delay_ms;
        self
    }

    /// 최대 대기 시간 설정
    pub fn with_max_delay(mut self, delay_ms: u64) -> Self {
        self.max_delay_ms = delay_ms;
        self
    }

    /// N번째 재시도 대기 시간 계산
    pub fn delay_for_attempt(&self, attempt: u32) -> u64 {
        let delay = self.initial_delay_ms as f64 * self.backoff_multiplier.powi(attempt as i32);
        (delay as u64).min(self.max_delay_ms)
    }

    /// 해당 상태 코드가 재시도 대상인지 확인
    pub fn should_retry_status(&self, status: u16) -> bool {
        self.retry_status_codes.contains(&status)
    }
}

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
    /// 프록시 설정
    proxy: Option<ProxyConfig>,
    /// 재시도 설정
    retry: Option<RetryConfig>,
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
            proxy: None,
            retry: None,
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

    /// 프록시 설정
    pub fn with_proxy(mut self, proxy: ProxyConfig) -> Self {
        self.proxy = Some(proxy);
        self
    }

    /// HTTP 프록시 간편 설정
    pub fn with_http_proxy(mut self, proxy_url: impl Into<String>) -> Self {
        self.proxy = Some(ProxyConfig::new().with_http_proxy(proxy_url));
        self
    }

    /// SOCKS5 프록시 간편 설정
    pub fn with_socks_proxy(mut self, proxy_url: impl Into<String>) -> Self {
        self.proxy = Some(ProxyConfig::new().with_socks_proxy(proxy_url));
        self
    }

    /// 재시도 설정
    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = Some(retry);
        self
    }

    /// 재시도 횟수 간편 설정
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        let retry = self.retry.unwrap_or_default();
        self.retry = Some(RetryConfig {
            max_retries,
            ..retry
        });
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

    /// 프록시 설정 조회
    pub fn proxy(&self) -> Option<&ProxyConfig> {
        self.proxy.as_ref()
    }

    /// 재시도 설정 조회
    pub fn retry(&self) -> Option<&RetryConfig> {
        self.retry.as_ref()
    }

    /// 재시도 설정 조회 (기본값 포함)
    pub fn retry_config(&self) -> RetryConfig {
        self.retry.clone().unwrap_or_default()
    }

    /// 프록시 설정 여부 확인
    pub fn has_proxy(&self) -> bool {
        self.proxy
            .as_ref()
            .map(|p| p.is_configured())
            .unwrap_or(false)
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
