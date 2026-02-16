//! WASM WebSocket Implementations
//!
//! This module contains WebSocket clients for various exchanges.

use wasm_bindgen::prelude::*;
use crate::client::{WsClient, WsConfig, WsEvent, Subscription};
use futures_util::StreamExt;
use super::utils::js_err;

/// WebSocket message types for JavaScript
#[wasm_bindgen]
#[derive(Clone)]
pub struct WasmWsMessage {
    #[wasm_bindgen(getter_with_clone)]
    pub message_type: String,
    #[wasm_bindgen(getter_with_clone)]
    pub symbol: String,
    #[wasm_bindgen(getter_with_clone)]
    pub data: String,
    pub timestamp: Option<i64>,
}

#[wasm_bindgen]
impl WasmWsMessage {
    /// Convert to JSON string
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"type":"{}","symbol":"{}","data":{},"timestamp":{}}}"#,
            self.message_type,
            self.symbol,
            self.data,
            self.timestamp.map_or("null".to_string(), |t| t.to_string())
        )
    }
}

/// Handle for WebSocket connection
///
/// Note: In WASM, connection lifecycle is managed by the browser.
/// The handle is a placeholder for future close/unsubscribe functionality.
#[wasm_bindgen]
pub struct WasmWsHandle {
    _marker: u8,
}

#[wasm_bindgen]
impl WasmWsHandle {
    /// Check if connection is active
    #[wasm_bindgen(js_name = isActive)]
    pub fn is_active(&self) -> bool {
        true
    }
}

// ============================================================================
// Binance WebSocket
// ============================================================================

/// Binance WebSocket client for WASM
#[wasm_bindgen]
pub struct WasmBinanceWs {
    base_url: String,
}

#[wasm_bindgen]
impl WasmBinanceWs {
    /// Create new Binance WebSocket instance
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            base_url: "wss://stream.binance.com:9443/ws".to_string(),
        }
    }

    /// Watch ticker updates via callback
    ///
    /// The callback receives a WasmWsMessage with ticker data
    #[wasm_bindgen(js_name = watchTicker)]
    pub fn watch_ticker(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "").to_lowercase();
        let stream = format!("{}@ticker", market_id);
        let url = format!("{}/{}", self.base_url, stream);

        let config = WsConfig {
            url: url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                            let ws_msg = WasmWsMessage {
                                message_type: "ticker".to_string(),
                                symbol: symbol_owned.clone(),
                                data: msg,
                                timestamp: data.get("E").and_then(|v| v.as_i64()),
                            };
                            let this = JsValue::null();
                            let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                        }
                    }
                    WsEvent::Error(err) => {
                        let ws_msg = WasmWsMessage {
                            message_type: "error".to_string(),
                            symbol: symbol_owned.clone(),
                            data: format!(r#"{{"error":"{}"}}"#, err),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Connected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "connected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"connected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Disconnected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "disconnected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"disconnected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    _ => {}
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }

    /// Watch order book updates via callback
    #[wasm_bindgen(js_name = watchOrderBook)]
    pub fn watch_order_book(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "").to_lowercase();
        let stream = format!("{}@depth@100ms", market_id);
        let url = format!("{}/{}", self.base_url, stream);

        let config = WsConfig {
            url: url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                            let ws_msg = WasmWsMessage {
                                message_type: "orderbook".to_string(),
                                symbol: symbol_owned.clone(),
                                data: msg,
                                timestamp: data.get("E").and_then(|v| v.as_i64()),
                            };
                            let this = JsValue::null();
                            let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                        }
                    }
                    WsEvent::Error(err) => {
                        let ws_msg = WasmWsMessage {
                            message_type: "error".to_string(),
                            symbol: symbol_owned.clone(),
                            data: format!(r#"{{"error":"{}"}}"#, err),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Connected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "connected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"connected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    _ => {}
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }

    /// Watch trades via callback
    #[wasm_bindgen(js_name = watchTrades)]
    pub fn watch_trades(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "").to_lowercase();
        let stream = format!("{}@trade", market_id);
        let url = format!("{}/{}", self.base_url, stream);

        let config = WsConfig {
            url: url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                if let WsEvent::Message(msg) = event {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                        let ws_msg = WasmWsMessage {
                            message_type: "trade".to_string(),
                            symbol: symbol_owned.clone(),
                            data: msg,
                            timestamp: data.get("E").and_then(|v| v.as_i64()),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }
}

// ============================================================================
// Upbit WebSocket
// ============================================================================

/// Helper to convert symbol to Upbit market ID
fn to_upbit_market_id(symbol: &str) -> String {
    let parts: Vec<&str> = symbol.split('/').collect();
    if parts.len() == 2 {
        format!("{}-{}", parts[1], parts[0])
    } else {
        symbol.to_string()
    }
}

/// Upbit WebSocket client for WASM
#[wasm_bindgen]
pub struct WasmUpbitWs {
    base_url: String,
}

#[wasm_bindgen]
impl WasmUpbitWs {
    /// Create new Upbit WebSocket instance
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            base_url: "wss://api.upbit.com/websocket/v1".to_string(),
        }
    }

    /// Watch ticker updates via callback
    #[wasm_bindgen(js_name = watchTicker)]
    pub fn watch_ticker(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = to_upbit_market_id(symbol);

        let config = WsConfig {
            url: self.base_url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        // Send subscription after connection
        let subscribe_msg = format!(
            r#"[{{"ticket":"wasm-ticker"}},{{"type":"ticker","codes":["{}"]}}]"#,
            market_id
        );

        let sub = Subscription {
            channel: "ticker".to_string(),
            symbol: Some(symbol.to_string()),
            subscribe_message: subscribe_msg,
            unsubscribe_message: None,
        };

        client.subscribe(sub).map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                            let ws_msg = WasmWsMessage {
                                message_type: "ticker".to_string(),
                                symbol: symbol_owned.clone(),
                                data: msg,
                                timestamp: data.get("timestamp").and_then(|v| v.as_i64()),
                            };
                            let this = JsValue::null();
                            let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                        }
                    }
                    WsEvent::Connected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "connected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"connected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Error(err) => {
                        let ws_msg = WasmWsMessage {
                            message_type: "error".to_string(),
                            symbol: symbol_owned.clone(),
                            data: format!(r#"{{"error":"{}"}}"#, err),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    _ => {}
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }

    /// Watch order book updates via callback
    #[wasm_bindgen(js_name = watchOrderBook)]
    pub fn watch_order_book(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = to_upbit_market_id(symbol);

        let config = WsConfig {
            url: self.base_url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let subscribe_msg = format!(
            r#"[{{"ticket":"wasm-orderbook"}},{{"type":"orderbook","codes":["{}"]}}]"#,
            market_id
        );

        let sub = Subscription {
            channel: "orderbook".to_string(),
            symbol: Some(symbol.to_string()),
            subscribe_message: subscribe_msg,
            unsubscribe_message: None,
        };

        client.subscribe(sub).map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                if let WsEvent::Message(msg) = event {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                        let ws_msg = WasmWsMessage {
                            message_type: "orderbook".to_string(),
                            symbol: symbol_owned.clone(),
                            data: msg,
                            timestamp: data.get("timestamp").and_then(|v| v.as_i64()),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }
}

// ============================================================================
// Bybit WebSocket
// ============================================================================

/// Bybit WebSocket client for WASM
#[wasm_bindgen]
pub struct WasmBybitWs {
    base_url: String,
}

#[wasm_bindgen]
impl WasmBybitWs {
    /// Create new Bybit WebSocket instance
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            base_url: "wss://stream.bybit.com/v5/public/spot".to_string(),
        }
    }

    /// Watch ticker updates via callback
    #[wasm_bindgen(js_name = watchTicker)]
    pub fn watch_ticker(&self, symbol: &str, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "");

        let config = WsConfig {
            url: self.base_url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let subscribe_msg = format!(
            r#"{{"op":"subscribe","args":["tickers.{}"]}}"#,
            market_id
        );

        let sub = Subscription {
            channel: "tickers".to_string(),
            symbol: Some(symbol.to_string()),
            subscribe_message: subscribe_msg.clone(),
            unsubscribe_message: Some(format!(
                r#"{{"op":"unsubscribe","args":["tickers.{}"]}}"#,
                market_id
            )),
        };

        client.subscribe(sub).map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                            // Skip subscription confirmation messages
                            if data.get("op").is_some() {
                                continue;
                            }
                            let ws_msg = WasmWsMessage {
                                message_type: "ticker".to_string(),
                                symbol: symbol_owned.clone(),
                                data: msg,
                                timestamp: data.get("ts").and_then(|v| v.as_i64()),
                            };
                            let this = JsValue::null();
                            let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                        }
                    }
                    WsEvent::Connected => {
                        let ws_msg = WasmWsMessage {
                            message_type: "connected".to_string(),
                            symbol: symbol_owned.clone(),
                            data: r#"{"status":"connected"}"#.to_string(),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    WsEvent::Error(err) => {
                        let ws_msg = WasmWsMessage {
                            message_type: "error".to_string(),
                            symbol: symbol_owned.clone(),
                            data: format!(r#"{{"error":"{}"}}"#, err),
                            timestamp: Some(js_sys::Date::now() as i64),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                    _ => {}
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }

    /// Watch order book updates via callback
    #[wasm_bindgen(js_name = watchOrderBook)]
    pub fn watch_order_book(&self, symbol: &str, depth: Option<u32>, callback: js_sys::Function) -> Result<WasmWsHandle, JsValue> {
        let market_id = symbol.replace("/", "");
        let depth_val = depth.unwrap_or(50);

        let config = WsConfig {
            url: self.base_url.clone(),
            ..Default::default()
        };

        let mut client = WsClient::new(config);
        let mut rx = client.connect().map_err(js_err)?;

        let subscribe_msg = format!(
            r#"{{"op":"subscribe","args":["orderbook.{}.{}"]}}"#,
            depth_val, market_id
        );

        let sub = Subscription {
            channel: "orderbook".to_string(),
            symbol: Some(symbol.to_string()),
            subscribe_message: subscribe_msg,
            unsubscribe_message: Some(format!(
                r#"{{"op":"unsubscribe","args":["orderbook.{}.{}"]}}"#,
                depth_val, market_id
            )),
        };

        client.subscribe(sub).map_err(js_err)?;

        let symbol_owned = symbol.to_string();
        let callback_clone = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = rx.next().await {
                if let WsEvent::Message(msg) = event {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg) {
                        // Skip subscription confirmation messages
                        if data.get("op").is_some() {
                            continue;
                        }
                        let ws_msg = WasmWsMessage {
                            message_type: "orderbook".to_string(),
                            symbol: symbol_owned.clone(),
                            data: msg,
                            timestamp: data.get("ts").and_then(|v| v.as_i64()),
                        };
                        let this = JsValue::null();
                        let _ = callback_clone.call1(&this, &JsValue::from(ws_msg));
                    }
                }
            }
        });

        Ok(WasmWsHandle { _marker: 0 })
    }
}
