//! HTTP client for API requests

use reqwest::Client;
use serde::de::DeserializeOwned;
use shared_types::error::{TradingError, TradingResult};
use std::collections::HashMap;
use std::time::Duration;

use super::ExchangeConfig;

/// HTTP 클라이언트
pub struct HttpClient {
    client: Client,
    base_url: String,
}

impl HttpClient {
    /// 새로운 HTTP 클라이언트 생성
    pub fn new(base_url: impl Into<String>, config: &ExchangeConfig) -> TradingResult<Self> {
        let base_url_str = base_url.into();
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms()))
            .build()
            .map_err(|e| TradingError::NetworkError {
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
    ) -> TradingResult<T> {
        let url = format!("{}{}", self.base_url, path);
        
        let mut request = self.client.get(&url);
        
        if let Some(params) = params {
            request = request.query(&params);
        }
        
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| TradingError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        if !response.status().is_success() {
            return Err(TradingError::ExchangeError {
                exchange: shared_types::common::Exchange::Upbit, // TODO: 동적으로 설정
                message: format!("HTTP {}: {}", response.status(), url),
            });
        }

        response.json().await.map_err(|e| TradingError::ParseError {
            data_type: std::any::type_name::<T>().to_string(),
            message: e.to_string(),
        })
    }

    /// POST 요청
    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
    ) -> TradingResult<T> {
        let url = format!("{}{}", self.base_url, path);
        
        let mut request = self.client.post(&url);
        
        if let Some(body) = body {
            request = request.json(&body);
        }
        
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| TradingError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        if !response.status().is_success() {
            return Err(TradingError::ExchangeError {
                exchange: shared_types::common::Exchange::Upbit, // TODO: 동적으로 설정
                message: format!("HTTP {}: {}", response.status(), url),
            });
        }

        response.json().await.map_err(|e| TradingError::ParseError {
            data_type: std::any::type_name::<T>().to_string(),
            message: e.to_string(),
        })
    }

    /// DELETE 요청
    pub async fn delete<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
        headers: Option<HashMap<String, String>>,
    ) -> TradingResult<T> {
        let url = format!("{}{}", self.base_url, path);
        
        let mut request = self.client.delete(&url);
        
        if let Some(params) = params {
            request = request.query(&params);
        }
        
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, &value);
            }
        }

        let response = request.send().await.map_err(|e| TradingError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        if !response.status().is_success() {
            return Err(TradingError::ExchangeError {
                exchange: shared_types::common::Exchange::Upbit,
                message: format!("HTTP {}: {}", response.status(), url),
            });
        }

        response.json().await.map_err(|e| TradingError::ParseError {
            data_type: std::any::type_name::<T>().to_string(),
            message: e.to_string(),
        })
    }
}
