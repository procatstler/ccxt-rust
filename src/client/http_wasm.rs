//! WASM-compatible HTTP client for browser environment

use gloo_net::http::{Request, RequestBuilder};
use serde::de::DeserializeOwned;
use std::collections::HashMap;

use super::config::ExchangeConfig;
use crate::errors::{CcxtError, CcxtResult};

/// WASM HTTP Client
pub struct HttpClient {
    base_url: String,
}

impl HttpClient {
    /// Build URL helper
    #[inline]
    fn build_url(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        }
    }

    /// Apply headers helper
    #[inline]
    fn apply_headers(
        mut request: RequestBuilder,
        headers: Option<HashMap<String, String>>,
    ) -> RequestBuilder {
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }
        request
    }

    /// Create new HTTP client
    pub fn new(base_url: impl Into<String>, _config: &ExchangeConfig) -> CcxtResult<Self> {
        Ok(Self {
            base_url: base_url.into(),
        })
    }

    /// GET request
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let mut url = self.build_url(path);

        // Add query params
        if let Some(params) = params {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            if !query.is_empty() {
                url = format!("{}?{}", url, query);
            }
        }

        let request = Request::get(&url);
        let request = Self::apply_headers(request, headers);

        let response = request.send().await.map_err(|e| CcxtError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        self.handle_response(response, &url).await
    }

    /// POST request
    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = Request::post(&url);

        request = Self::apply_headers(request, headers);
        request = request.header("Content-Type", "application/json");

        let response = if let Some(body) = body {
            request
                .body(serde_json::to_string(&body).unwrap_or_default())
                .map_err(|e| CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                })?
                .send()
                .await
        } else {
            request.send().await
        }
        .map_err(|e| CcxtError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        self.handle_response(response, &url).await
    }

    /// POST form request
    pub async fn post_form<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = Request::post(&url);

        request = Self::apply_headers(request, headers);
        request = request.header("Content-Type", "application/x-www-form-urlencoded");

        let body: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let response = request
            .body(body)
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        self.handle_response(response, &url).await
    }

    /// DELETE request
    pub async fn delete<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let mut url = self.build_url(path);

        if let Some(params) = params {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            if !query.is_empty() {
                url = format!("{}?{}", url, query);
            }
        }

        let request = Request::delete(&url);
        let request = Self::apply_headers(request, headers);

        let response = request.send().await.map_err(|e| CcxtError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        self.handle_response(response, &url).await
    }

    /// PUT request
    pub async fn put<T: DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = Request::put(&url);

        request = Self::apply_headers(request, headers);
        request = request.header("Content-Type", "application/x-www-form-urlencoded");

        let body: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let response = request
            .body(body)
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        self.handle_response(response, &url).await
    }

    /// PUT JSON request
    pub async fn put_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = Request::put(&url);

        request = Self::apply_headers(request, headers);
        request = request.header("Content-Type", "application/json");

        let response = if let Some(body) = body {
            request
                .body(serde_json::to_string(&body).unwrap_or_default())
                .map_err(|e| CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                })?
                .send()
                .await
        } else {
            request.send().await
        }
        .map_err(|e| CcxtError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        self.handle_response(response, &url).await
    }

    /// DELETE JSON request
    pub async fn delete_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = Request::delete(&url);

        request = Self::apply_headers(request, headers);
        request = request.header("Content-Type", "application/json");

        let response = if let Some(body) = body {
            request
                .body(serde_json::to_string(&body).unwrap_or_default())
                .map_err(|e| CcxtError::NetworkError {
                    url: url.clone(),
                    message: e.to_string(),
                })?
                .send()
                .await
        } else {
            request.send().await
        }
        .map_err(|e| CcxtError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        self.handle_response(response, &url).await
    }

    /// PATCH request
    pub async fn patch<T: DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        let url = self.build_url(path);
        let mut request = Request::patch(&url);

        request = Self::apply_headers(request, headers);
        request = request.header("Content-Type", "application/json");

        let body = serde_json::to_string(&params).unwrap_or_default();

        let response = request
            .body(body)
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        self.handle_response(response, &url).await
    }

    /// Handle response
    async fn handle_response<T: DeserializeOwned>(
        &self,
        response: gloo_net::http::Response,
        url: &str,
    ) -> CcxtResult<T> {
        let status = response.status();

        if status < 200 || status >= 300 {
            let body = response.text().await.unwrap_or_default();

            return Err(match status {
                401 => CcxtError::AuthenticationError { message: body },
                403 => CcxtError::PermissionDenied { message: body },
                404 => CcxtError::BadRequest {
                    message: format!("Not found: {url}"),
                },
                429 => CcxtError::RateLimitExceeded {
                    message: body,
                    retry_after_ms: None,
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
