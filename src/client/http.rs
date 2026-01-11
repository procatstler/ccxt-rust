//! HTTP client for API requests

use reqwest::{Client, Proxy};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::time::Duration;

use super::{ExchangeConfig, RetryConfig};
use crate::errors::{CcxtError, CcxtResult};

/// HTTP 클라이언트
pub struct HttpClient {
    client: Client,
    base_url: String,
    #[allow(dead_code)]
    retry_config: RetryConfig,
}

impl HttpClient {
    // ========================================================================
    // Private Helper Methods (DRY - Don't Repeat Yourself)
    // ========================================================================

    /// URL 빌드 헬퍼 - 절대 URL인 경우 그대로, 상대 경로인 경우 base_url과 결합
    #[inline]
    fn build_url(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        }
    }

    /// 헤더 적용 헬퍼 - Option<HashMap>을 RequestBuilder에 적용
    #[inline]
    fn apply_headers(
        mut request: reqwest::RequestBuilder,
        headers: Option<HashMap<String, String>>,
    ) -> reqwest::RequestBuilder {
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }
        request
    }

    /// 요청 전송 및 에러 처리 헬퍼
    #[inline]
    async fn send_request(
        request: reqwest::RequestBuilder,
        url: &str,
    ) -> CcxtResult<reqwest::Response> {
        request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout {
                    url: url.to_string(),
                }
            } else {
                CcxtError::NetworkError {
                    url: url.to_string(),
                    message: e.to_string(),
                }
            }
        })
    }

    // ========================================================================
    // Public API
    // ========================================================================

    /// 새로운 HTTP 클라이언트 생성
    pub fn new(base_url: impl Into<String>, config: &ExchangeConfig) -> CcxtResult<Self> {
        let base_url_str = base_url.into();
        let mut builder = Client::builder().timeout(Duration::from_millis(config.timeout_ms()));

        // 프록시 설정 적용
        if let Some(proxy_config) = config.proxy() {
            if let Some(http_proxy) = &proxy_config.http_proxy {
                let mut proxy = Proxy::all(http_proxy).map_err(|e| CcxtError::NetworkError {
                    url: base_url_str.clone(),
                    message: format!("Invalid proxy URL: {e}"),
                })?;

                // 프록시 인증 설정
                if let (Some(user), Some(pass)) =
                    (&proxy_config.proxy_username, &proxy_config.proxy_password)
                {
                    proxy = proxy.basic_auth(user, pass);
                }

                builder = builder.proxy(proxy);
            }

            // SOCKS5 프록시 (reqwest socks 기능 필요)
            // Note: SOCKS proxy support requires reqwest "socks" feature
            if let Some(socks_proxy) = &proxy_config.socks_proxy {
                let mut proxy = Proxy::all(socks_proxy).map_err(|e| CcxtError::NetworkError {
                    url: base_url_str.clone(),
                    message: format!("Invalid SOCKS proxy URL: {e}"),
                })?;

                if let (Some(user), Some(pass)) =
                    (&proxy_config.proxy_username, &proxy_config.proxy_password)
                {
                    proxy = proxy.basic_auth(user, pass);
                }

                builder = builder.proxy(proxy);
            }
        }

        let client = builder.build().map_err(|e| CcxtError::NetworkError {
            url: base_url_str.clone(),
            message: e.to_string(),
        })?;

        Ok(Self {
            client,
            base_url: base_url_str,
            retry_config: config.retry_config(),
        })
    }

    /// 재시도 로직을 포함한 요청 실행
    #[allow(dead_code)]
    async fn execute_with_retry<F, Fut, T>(&self, url: &str, operation: F) -> CcxtResult<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
        T: DeserializeOwned,
    {
        let mut last_error = None;

        for attempt in 0..=self.retry_config.max_retries {
            if attempt > 0 {
                let delay = self.retry_config.delay_for_attempt(attempt - 1);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }

            match operation().await {
                Ok(response) => {
                    let status = response.status();

                    // 재시도 가능한 상태 코드인지 확인
                    if !status.is_success()
                        && self.retry_config.should_retry_status(status.as_u16())
                        && attempt < self.retry_config.max_retries
                    {
                        // 429의 경우 Retry-After 헤더 확인
                        if status.as_u16() == 429 {
                            if let Some(retry_after) = response
                                .headers()
                                .get("Retry-After")
                                .and_then(|v| v.to_str().ok())
                                .and_then(|s| s.parse::<u64>().ok())
                            {
                                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                                continue;
                            }
                        }

                        let body = response.text().await.unwrap_or_default();
                        last_error = Some(CcxtError::ExchangeError {
                            message: format!("HTTP {status}: {body}"),
                        });
                        continue;
                    }

                    return self.handle_response(response, url).await;
                },
                Err(e) => {
                    // 네트워크 에러 재시도
                    if self.retry_config.retry_on_network_error
                        && attempt < self.retry_config.max_retries
                    {
                        last_error = Some(if e.is_timeout() {
                            CcxtError::RequestTimeout {
                                url: url.to_string(),
                            }
                        } else {
                            CcxtError::NetworkError {
                                url: url.to_string(),
                                message: e.to_string(),
                            }
                        });
                        continue;
                    }

                    return Err(if e.is_timeout() {
                        CcxtError::RequestTimeout {
                            url: url.to_string(),
                        }
                    } else {
                        CcxtError::NetworkError {
                            url: url.to_string(),
                            message: e.to_string(),
                        }
                    });
                },
            }
        }

        Err(last_error.unwrap_or_else(|| CcxtError::NetworkError {
            url: url.to_string(),
            message: "Max retries exceeded".into(),
        }))
    }

    /// GET 요청
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = self.client.get(&url);

        if let Some(params) = params {
            request = request.query(&params);
        }

        request = Self::apply_headers(request, headers);
        let response = Self::send_request(request, &url).await?;
        self.handle_response(response, &url).await
    }

    /// POST 요청
    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = self.client.post(&url);

        if let Some(body) = body {
            request = request.json(&body);
        }

        request = Self::apply_headers(request, headers);
        let response = Self::send_request(request, &url).await?;
        self.handle_response(response, &url).await
    }

    /// POST 요청 (form-urlencoded body)
    pub async fn post_form<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let request = self.client.post(&url).form(params);
        let request = Self::apply_headers(request, headers);
        let response = Self::send_request(request, &url).await?;
        self.handle_response(response, &url).await
    }

    /// DELETE 요청
    pub async fn delete<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = self.client.delete(&url);

        if let Some(params) = params {
            request = request.query(&params);
        }

        request = Self::apply_headers(request, headers);
        let response = Self::send_request(request, &url).await?;
        self.handle_response(response, &url).await
    }

    /// PUT 요청
    pub async fn put<T: DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let request = self.client.put(&url).form(&params);
        let request = Self::apply_headers(request, headers);
        let response = Self::send_request(request, &url).await?;
        self.handle_response(response, &url).await
    }

    /// PUT 요청 (JSON body)
    pub async fn put_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = self.client.put(&url);

        if let Some(body) = body {
            request = request.json(&body);
        }

        request = Self::apply_headers(request, headers);
        let response = Self::send_request(request, &url).await?;
        self.handle_response(response, &url).await
    }

    /// DELETE 요청 (JSON body)
    pub async fn delete_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = self.client.delete(&url);

        if let Some(body) = body {
            request = request.json(&body);
        }

        request = Self::apply_headers(request, headers);
        let response = Self::send_request(request, &url).await?;
        self.handle_response(response, &url).await
    }

    /// PATCH 요청
    pub async fn patch<T: DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let request = self.client.patch(&url).json(&params);
        let request = Self::apply_headers(request, headers);
        let response = Self::send_request(request, &url).await?;
        self.handle_response(response, &url).await
    }

    /// 응답 처리
    async fn handle_response<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
        url: &str,
    ) -> CcxtResult<T> {
        let status = response.status();

        if !status.is_success() {
            // Extract Retry-After header before consuming the response
            let retry_after_ms = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(|secs| secs * 1000); // Convert seconds to ms

            let body = response.text().await.unwrap_or_default();

            // 에러 타입 판별
            return Err(match status.as_u16() {
                401 => CcxtError::AuthenticationError { message: body },
                403 => CcxtError::PermissionDenied { message: body },
                404 => CcxtError::BadRequest {
                    message: format!("Not found: {url}"),
                },
                429 => CcxtError::RateLimitExceeded {
                    message: body,
                    retry_after_ms,
                },
                500..=599 => CcxtError::ExchangeNotAvailable { message: body },
                _ => CcxtError::ExchangeError {
                    message: format!("HTTP {status}: {body}"),
                },
            });
        }

        response.json().await.map_err(|e| CcxtError::ParseError {
            data_type: std::any::type_name::<T>().to_string(),
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_client_creation() {
        let config = ExchangeConfig::new();
        let client = HttpClient::new("https://api.example.com", &config);
        assert!(client.is_ok());
    }
}
