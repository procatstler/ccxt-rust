//! WASM-compatible WebSocket client using web-sys
//!
//! Browser-native WebSocket implementation for WASM target

use futures_channel::mpsc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

use crate::errors::{CcxtError, CcxtResult};

/// WebSocket event
#[derive(Debug, Clone)]
pub enum WsEvent {
    Connected,
    Disconnected,
    Reconnecting {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
    },
    Reconnected {
        attempts: u32,
    },
    Message(String),
    Error(String),
    Ping,
    Pong,
    HealthOk,
    HealthWarning {
        last_pong_ago_secs: u64,
    },
}

/// WebSocket command
#[derive(Debug)]
pub enum WsCommand {
    Send(String),
    Close,
    Ping,
}

/// Subscription info
#[derive(Debug, Clone)]
pub struct Subscription {
    pub channel: String,
    pub symbol: Option<String>,
    pub subscribe_message: String,
    pub unsubscribe_message: Option<String>,
}

/// Reconnect configuration
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    pub enabled: bool,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
    pub max_attempts: u32,
    pub jitter: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            max_attempts: 10,
            jitter: 0.1,
        }
    }
}

impl ReconnectConfig {
    pub fn calculate_delay(&self, attempt: u32) -> u64 {
        let base_delay =
            self.initial_delay_ms as f64 * self.backoff_multiplier.powi(attempt as i32);
        let delay = base_delay.min(self.max_delay_ms as f64);

        if self.jitter > 0.0 {
            let jitter_range = delay * self.jitter;
            let random = js_sys::Math::random();
            let jitter = (random * jitter_range * 2.0) - jitter_range;
            (delay + jitter).max(0.0) as u64
        } else {
            delay as u64
        }
    }
}

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    pub enabled: bool,
    pub ping_interval_secs: u64,
    pub pong_timeout_secs: u64,
    pub max_missed_pongs: u32,
    pub warning_threshold_secs: u64,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ping_interval_secs: 30,
            pong_timeout_secs: 10,
            max_missed_pongs: 3,
            warning_threshold_secs: 45,
        }
    }
}

/// WebSocket client configuration
#[derive(Debug, Clone)]
pub struct WsConfig {
    pub url: String,
    pub connect_timeout_secs: u64,
    pub reconnect: ReconnectConfig,
    pub health_check: HealthCheckConfig,
    pub subscription_delay_ms: u64,
    pub message_buffer_size: usize,
    pub auto_reconnect: bool,
    pub reconnect_interval_ms: u64,
    pub max_reconnect_attempts: u32,
    pub ping_interval_secs: u64,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            connect_timeout_secs: 30,
            reconnect: ReconnectConfig::default(),
            health_check: HealthCheckConfig::default(),
            subscription_delay_ms: 100,
            message_buffer_size: 1000,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
        }
    }
}

impl WsConfig {
    pub fn with_reconnect(mut self, config: ReconnectConfig) -> Self {
        self.reconnect = config;
        self
    }

    pub fn with_health_check(mut self, config: HealthCheckConfig) -> Self {
        self.health_check = config;
        self
    }

    pub fn without_reconnect(mut self) -> Self {
        self.reconnect.enabled = false;
        self.auto_reconnect = false;
        self
    }
}

/// WebSocket metrics
#[derive(Debug, Default)]
pub struct WsMetrics {
    pub total_connections: AtomicUsize,
    pub total_reconnections: AtomicUsize,
    pub consecutive_failures: AtomicUsize,
    pub messages_sent: AtomicU64,
    pub messages_received: AtomicU64,
    pub last_connected_at: AtomicU64,
    pub last_pong_at: AtomicU64,
    pub last_message_at: AtomicU64,
}

impl WsMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_connection(&self) {
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.last_connected_at
            .store(js_sys::Date::now() as u64, Ordering::Relaxed);
    }

    pub fn record_reconnection(&self) {
        self.total_reconnections.fetch_add(1, Ordering::Relaxed);
        self.record_connection();
    }

    pub fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_message_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_message_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        self.last_message_at
            .store(js_sys::Date::now() as u64, Ordering::Relaxed);
    }

    pub fn record_pong(&self) {
        self.last_pong_at
            .store(js_sys::Date::now() as u64, Ordering::Relaxed);
    }

    pub fn seconds_since_last_pong(&self) -> u64 {
        let last_pong = self.last_pong_at.load(Ordering::Relaxed);
        if last_pong == 0 {
            return u64::MAX;
        }
        let now = js_sys::Date::now() as u64;
        (now.saturating_sub(last_pong)) / 1000
    }

    pub fn snapshot(&self) -> WsMetricsSnapshot {
        WsMetricsSnapshot {
            total_connections: self.total_connections.load(Ordering::Relaxed),
            total_reconnections: self.total_reconnections.load(Ordering::Relaxed),
            consecutive_failures: self.consecutive_failures.load(Ordering::Relaxed),
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            messages_received: self.messages_received.load(Ordering::Relaxed),
            last_connected_at: self.last_connected_at.load(Ordering::Relaxed),
            last_pong_at: self.last_pong_at.load(Ordering::Relaxed),
            last_message_at: self.last_message_at.load(Ordering::Relaxed),
        }
    }
}

/// WebSocket metrics snapshot
#[derive(Debug, Clone)]
pub struct WsMetricsSnapshot {
    pub total_connections: usize,
    pub total_reconnections: usize,
    pub consecutive_failures: usize,
    pub messages_sent: u64,
    pub messages_received: u64,
    pub last_connected_at: u64,
    pub last_pong_at: u64,
    pub last_message_at: u64,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Closed,
}

/// WebSocket client for WASM
pub struct WsClient {
    config: WsConfig,
    ws: Rc<RefCell<Option<WebSocket>>>,
    state: Rc<RefCell<ConnectionState>>,
    subscriptions: Rc<RefCell<HashMap<String, Subscription>>>,
    event_tx: Rc<RefCell<Option<mpsc::UnboundedSender<WsEvent>>>>,
    metrics: Rc<WsMetrics>,
    // Store closures to prevent them from being dropped
    _onopen: Rc<RefCell<Option<Closure<dyn FnMut()>>>>,
    _onclose: Rc<RefCell<Option<Closure<dyn FnMut(CloseEvent)>>>>,
    _onmessage: Rc<RefCell<Option<Closure<dyn FnMut(MessageEvent)>>>>,
    _onerror: Rc<RefCell<Option<Closure<dyn FnMut(ErrorEvent)>>>>,
}

impl WsClient {
    /// Create new WebSocket client
    pub fn new(config: WsConfig) -> Self {
        Self {
            config,
            ws: Rc::new(RefCell::new(None)),
            state: Rc::new(RefCell::new(ConnectionState::Disconnected)),
            subscriptions: Rc::new(RefCell::new(HashMap::new())),
            event_tx: Rc::new(RefCell::new(None)),
            metrics: Rc::new(WsMetrics::new()),
            _onopen: Rc::new(RefCell::new(None)),
            _onclose: Rc::new(RefCell::new(None)),
            _onmessage: Rc::new(RefCell::new(None)),
            _onerror: Rc::new(RefCell::new(None)),
        }
    }

    /// Create client with URL
    pub fn with_url(url: &str) -> Self {
        Self::new(WsConfig {
            url: url.to_string(),
            ..Default::default()
        })
    }

    /// Get connection state
    pub fn state(&self) -> ConnectionState {
        *self.state.borrow()
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        *self.state.borrow() == ConnectionState::Connected
    }

    /// Get metrics
    pub fn metrics(&self) -> &WsMetrics {
        &self.metrics
    }

    /// Get metrics snapshot
    pub fn metrics_snapshot(&self) -> WsMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Connect to WebSocket server
    pub fn connect(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsEvent>> {
        let (event_tx, event_rx) = mpsc::unbounded();
        *self.event_tx.borrow_mut() = Some(event_tx);

        self.create_connection()?;

        Ok(event_rx)
    }

    /// Internal: create WebSocket connection
    fn create_connection(&self) -> CcxtResult<()> {
        *self.state.borrow_mut() = ConnectionState::Connecting;

        let ws = WebSocket::new(&self.config.url).map_err(|e| CcxtError::NetworkError {
            url: self.config.url.clone(),
            message: format!("Failed to create WebSocket: {:?}", e),
        })?;

        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        // Setup event handlers
        self.setup_handlers(&ws);

        *self.ws.borrow_mut() = Some(ws);
        Ok(())
    }

    /// Setup WebSocket event handlers
    fn setup_handlers(&self, ws: &WebSocket) {
        let event_tx = self.event_tx.clone();
        let state = self.state.clone();
        let metrics = self.metrics.clone();
        let subscriptions = self.subscriptions.clone();
        let ws_ref = self.ws.clone();

        // onopen handler
        let onopen_tx = event_tx.clone();
        let onopen_state = state.clone();
        let onopen_metrics = metrics.clone();
        let onopen_subs = subscriptions.clone();
        let onopen_ws = ws_ref.clone();
        let onopen = Closure::wrap(Box::new(move || {
            *onopen_state.borrow_mut() = ConnectionState::Connected;
            onopen_metrics.record_connection();

            if let Some(tx) = onopen_tx.borrow().as_ref() {
                let _ = tx.unbounded_send(WsEvent::Connected);
            }

            // Restore subscriptions
            if let Some(ws) = onopen_ws.borrow().as_ref() {
                for (_, sub) in onopen_subs.borrow().iter() {
                    let _ = ws.send_with_str(&sub.subscribe_message);
                }
            }
        }) as Box<dyn FnMut()>);
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        *self._onopen.borrow_mut() = Some(onopen);

        // onclose handler
        let onclose_tx = event_tx.clone();
        let onclose_state = state.clone();
        let onclose = Closure::wrap(Box::new(move |_e: CloseEvent| {
            *onclose_state.borrow_mut() = ConnectionState::Disconnected;
            if let Some(tx) = onclose_tx.borrow().as_ref() {
                let _ = tx.unbounded_send(WsEvent::Disconnected);
            }
        }) as Box<dyn FnMut(CloseEvent)>);
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        *self._onclose.borrow_mut() = Some(onclose);

        // onmessage handler
        let onmessage_tx = event_tx.clone();
        let onmessage_metrics = metrics.clone();
        let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
            onmessage_metrics.record_message_received();

            if let Ok(text) = e.data().dyn_into::<js_sys::JsString>() {
                let msg: String = text.into();
                if let Some(tx) = onmessage_tx.borrow().as_ref() {
                    let _ = tx.unbounded_send(WsEvent::Message(msg));
                }
            } else if let Ok(array_buffer) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
                let array = js_sys::Uint8Array::new(&array_buffer);
                let mut vec = vec![0u8; array.length() as usize];
                array.copy_to(&mut vec);
                if let Ok(text) = String::from_utf8(vec) {
                    if let Some(tx) = onmessage_tx.borrow().as_ref() {
                        let _ = tx.unbounded_send(WsEvent::Message(text));
                    }
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        *self._onmessage.borrow_mut() = Some(onmessage);

        // onerror handler
        let onerror_tx = event_tx;
        let onerror_metrics = metrics;
        let onerror = Closure::wrap(Box::new(move |e: ErrorEvent| {
            onerror_metrics.record_failure();
            if let Some(tx) = onerror_tx.borrow().as_ref() {
                let _ = tx.unbounded_send(WsEvent::Error(e.message()));
            }
        }) as Box<dyn FnMut(ErrorEvent)>);
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        *self._onerror.borrow_mut() = Some(onerror);
    }

    /// Send message
    pub fn send(&self, message: &str) -> CcxtResult<()> {
        if let Some(ws) = self.ws.borrow().as_ref() {
            ws.send_with_str(message)
                .map_err(|e| CcxtError::NetworkError {
                    url: self.config.url.clone(),
                    message: format!("Failed to send message: {:?}", e),
                })?;
            self.metrics.record_message_sent();
        }
        Ok(())
    }

    /// Subscribe
    pub fn subscribe(&self, subscription: Subscription) -> CcxtResult<()> {
        let key = format!(
            "{}:{}",
            subscription.channel,
            subscription.symbol.as_deref().unwrap_or("")
        );

        self.send(&subscription.subscribe_message)?;
        self.subscriptions.borrow_mut().insert(key, subscription);

        Ok(())
    }

    /// Unsubscribe
    pub fn unsubscribe(&self, channel: &str, symbol: Option<&str>) -> CcxtResult<()> {
        let key = format!("{}:{}", channel, symbol.unwrap_or(""));

        if let Some(sub) = self.subscriptions.borrow_mut().remove(&key) {
            if let Some(unsub_msg) = sub.unsubscribe_message {
                self.send(&unsub_msg)?;
            }
        }

        Ok(())
    }

    /// Close connection
    pub fn close(&self) -> CcxtResult<()> {
        if let Some(ws) = self.ws.borrow().as_ref() {
            ws.close().map_err(|e| CcxtError::NetworkError {
                url: self.config.url.clone(),
                message: format!("Failed to close connection: {:?}", e),
            })?;
        }
        *self.state.borrow_mut() = ConnectionState::Closed;
        Ok(())
    }
}
