//! HTTP client for API requests

use reqwest::Client;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::time::Duration;

use crate::errors::{CcxtError, CcxtResult};
use super::ExchangeConfig;

/// HTTP 클라이언트
pub struct HttpClient {
    client: Client,
    base_url: String,
}

impl HttpClient {
    /// 새로운 HTTP 클라이언트 생성
    pub fn new(base_url: impl Into<String>, config: &ExchangeConfig) -> CcxtResult<Self> {
        let base_url_str = base_url.into();
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms()))
            .build()
            .map_err(|e| CcxtError::NetworkError {
                url: base_url_str.clone(),
                message: e.to_string(),
            })?;

        Ok(Self {
            client,
            base_url: base_url_str,
        })
    }

    /// GET 요청
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        };

        let mut request = self.client.get(&url);

        if let Some(params) = params {
            request = request.query(&params);
        }

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout { url: url.clone() }
            } else {
                CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                }
            }
        })?;

        self.handle_response(response, &url).await
    }

    /// POST 요청
    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        };

        let mut request = self.client.post(&url);

        if let Some(body) = body {
            request = request.json(&body);
        }

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout { url: url.clone() }
            } else {
                CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                }
            }
        })?;

        self.handle_response(response, &url).await
    }

    /// POST 요청 (form-urlencoded body)
    pub async fn post_form<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        };

        let mut request = self.client.post(&url).form(params);

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout { url: url.clone() }
            } else {
                CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                }
            }
        })?;

        self.handle_response(response, &url).await
    }

    /// DELETE 요청
    pub async fn delete<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        };

        let mut request = self.client.delete(&url);

        if let Some(params) = params {
            request = request.query(&params);
        }

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout { url: url.clone() }
            } else {
                CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                }
            }
        })?;

        self.handle_response(response, &url).await
    }

    /// PUT 요청
    pub async fn put<T: DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        };

        let mut request = self.client.put(&url).form(&params);

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout { url: url.clone() }
            } else {
                CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                }
            }
        })?;

        self.handle_response(response, &url).await
    }

    /// PUT 요청 (JSON body)
    pub async fn put_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        };

        let mut request = self.client.put(&url);

        if let Some(body) = body {
            request = request.json(&body);
        }

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout { url: url.clone() }
            } else {
                CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                }
            }
        })?;

        self.handle_response(response, &url).await
    }

    /// DELETE 요청 (JSON body)
    pub async fn delete_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        };

        let mut request = self.client.delete(&url);

        if let Some(body) = body {
            request = request.json(&body);
        }

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout { url: url.clone() }
            } else {
                CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                }
            }
        })?;

        self.handle_response(response, &url).await
    }

    /// PATCH 요청
    pub async fn patch<T: DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        };

        let mut request = self.client.patch(&url).json(&params);

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CcxtError::RequestTimeout { url: url.clone() }
            } else {
                CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                }
            }
        })?;

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
                401 => CcxtError::AuthenticationError {
                    message: body,
                },
                403 => CcxtError::PermissionDenied {
                    message: body,
                },
                404 => CcxtError::BadRequest {
                    message: format!("Not found: {url}"),
                },
                429 => CcxtError::RateLimitExceeded {
                    message: body,
                    retry_after_ms,
                },
                500..=599 => CcxtError::ExchangeNotAvailable {
                    message: body,
                },
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
