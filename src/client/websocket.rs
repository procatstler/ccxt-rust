//! WebSocket Client
//!
//! WebSocket 연결 관리 및 메시지 처리

use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::Message,
};

use crate::errors::{CcxtError, CcxtResult};

/// WebSocket 이벤트
#[derive(Debug, Clone)]
pub enum WsEvent {
    /// 연결됨
    Connected,
    /// 연결 해제됨
    Disconnected,
    /// 메시지 수신
    Message(String),
    /// 에러 발생
    Error(String),
    /// Ping 수신
    Ping,
    /// Pong 수신
    Pong,
}

/// WebSocket 명령
#[derive(Debug)]
pub enum WsCommand {
    /// 메시지 전송
    Send(String),
    /// 연결 종료
    Close,
    /// Ping 전송
    Ping,
}

/// WebSocket 구독 정보
#[derive(Debug, Clone)]
pub struct Subscription {
    /// 채널 이름
    pub channel: String,
    /// 심볼
    pub symbol: Option<String>,
    /// 구독 메시지
    pub subscribe_message: String,
    /// 구독 해제 메시지
    pub unsubscribe_message: Option<String>,
}

/// WebSocket 클라이언트 설정
#[derive(Debug, Clone)]
pub struct WsConfig {
    /// WebSocket URL
    pub url: String,
    /// 자동 재연결 여부
    pub auto_reconnect: bool,
    /// 재연결 간격 (밀리초)
    pub reconnect_interval_ms: u64,
    /// 최대 재연결 시도 횟수
    pub max_reconnect_attempts: u32,
    /// Ping 간격 (초)
    pub ping_interval_secs: u64,
    /// 연결 타임아웃 (초)
    pub connect_timeout_secs: u64,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        }
    }
}

/// WebSocket 연결 상태
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Closed,
}

/// WebSocket 클라이언트
pub struct WsClient {
    config: WsConfig,
    state: Arc<RwLock<ConnectionState>>,
    subscriptions: Arc<RwLock<HashMap<String, Subscription>>>,
    event_tx: Option<mpsc::UnboundedSender<WsEvent>>,
    command_tx: Option<mpsc::UnboundedSender<WsCommand>>,
}

impl WsClient {
    /// 새 WebSocket 클라이언트 생성
    pub fn new(config: WsConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            command_tx: None,
        }
    }

    /// URL로 WebSocket 클라이언트 생성
    pub fn with_url(url: &str) -> Self {
        Self::new(WsConfig {
            url: url.to_string(),
            ..Default::default()
        })
    }

    /// 연결 상태 조회
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// 연결 여부 확인
    pub async fn is_connected(&self) -> bool {
        *self.state.read().await == ConnectionState::Connected
    }

    /// WebSocket 연결 시작
    /// 이벤트를 수신할 채널을 반환합니다.
    pub async fn connect(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsEvent>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        self.event_tx = Some(event_tx.clone());
        self.command_tx = Some(command_tx);

        let config = self.config.clone();
        let state = self.state.clone();
        let subscriptions = self.subscriptions.clone();

        // 연결 태스크 시작
        tokio::spawn(async move {
            Self::connection_loop(config, state, subscriptions, event_tx, command_rx).await;
        });

        Ok(event_rx)
    }

    /// 연결 루프
    async fn connection_loop(
        config: WsConfig,
        state: Arc<RwLock<ConnectionState>>,
        subscriptions: Arc<RwLock<HashMap<String, Subscription>>>,
        event_tx: mpsc::UnboundedSender<WsEvent>,
        mut command_rx: mpsc::UnboundedReceiver<WsCommand>,
    ) {
        let mut reconnect_attempts = 0;

        loop {
            // 연결 시도
            *state.write().await = ConnectionState::Connecting;

            let connect_result = timeout(
                Duration::from_secs(config.connect_timeout_secs),
                connect_async(&config.url),
            )
            .await;

            let (ws_stream, _) = match connect_result {
                Ok(Ok((stream, response))) => {
                    reconnect_attempts = 0;
                    *state.write().await = ConnectionState::Connected;
                    let _ = event_tx.send(WsEvent::Connected);
                    (stream, response)
                }
                Ok(Err(e)) => {
                    let _ = event_tx.send(WsEvent::Error(format!("Connection error: {e}")));

                    if config.auto_reconnect && reconnect_attempts < config.max_reconnect_attempts {
                        reconnect_attempts += 1;
                        *state.write().await = ConnectionState::Reconnecting;
                        tokio::time::sleep(Duration::from_millis(config.reconnect_interval_ms)).await;
                        continue;
                    } else {
                        *state.write().await = ConnectionState::Closed;
                        return;
                    }
                }
                Err(_) => {
                    let _ = event_tx.send(WsEvent::Error("Connection timeout".into()));

                    if config.auto_reconnect && reconnect_attempts < config.max_reconnect_attempts {
                        reconnect_attempts += 1;
                        *state.write().await = ConnectionState::Reconnecting;
                        tokio::time::sleep(Duration::from_millis(config.reconnect_interval_ms)).await;
                        continue;
                    } else {
                        *state.write().await = ConnectionState::Closed;
                        return;
                    }
                }
            };

            let (mut write, mut read) = ws_stream.split();

            // 기존 구독 복원
            {
                let subs = subscriptions.read().await;
                for (_, sub) in subs.iter() {
                    let _ = write.send(Message::Text(sub.subscribe_message.clone())).await;
                }
            }

            // Ping 타이머
            let mut ping_interval = interval(Duration::from_secs(config.ping_interval_secs));

            loop {
                tokio::select! {
                    // WebSocket 메시지 수신
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                let _ = event_tx.send(WsEvent::Message(text));
                            }
                            Some(Ok(Message::Binary(data))) => {
                                if let Ok(text) = String::from_utf8(data) {
                                    let _ = event_tx.send(WsEvent::Message(text));
                                }
                            }
                            Some(Ok(Message::Ping(_))) => {
                                let _ = event_tx.send(WsEvent::Ping);
                                let _ = write.send(Message::Pong(vec![])).await;
                            }
                            Some(Ok(Message::Pong(_))) => {
                                let _ = event_tx.send(WsEvent::Pong);
                            }
                            Some(Ok(Message::Close(_))) => {
                                let _ = event_tx.send(WsEvent::Disconnected);
                                break;
                            }
                            Some(Err(e)) => {
                                let _ = event_tx.send(WsEvent::Error(format!("WebSocket error: {e}")));
                                break;
                            }
                            None => {
                                let _ = event_tx.send(WsEvent::Disconnected);
                                break;
                            }
                            _ => {}
                        }
                    }

                    // 명령 수신
                    cmd = command_rx.recv() => {
                        match cmd {
                            Some(WsCommand::Send(msg)) => {
                                if write.send(Message::Text(msg)).await.is_err() {
                                    break;
                                }
                            }
                            Some(WsCommand::Ping) => {
                                if write.send(Message::Ping(vec![])).await.is_err() {
                                    break;
                                }
                            }
                            Some(WsCommand::Close) => {
                                let _ = write.send(Message::Close(None)).await;
                                *state.write().await = ConnectionState::Closed;
                                let _ = event_tx.send(WsEvent::Disconnected);
                                return;
                            }
                            None => {
                                break;
                            }
                        }
                    }

                    // Ping 타이머
                    _ = ping_interval.tick() => {
                        if write.send(Message::Ping(vec![])).await.is_err() {
                            break;
                        }
                    }
                }
            }

            // 연결 해제됨
            *state.write().await = ConnectionState::Disconnected;

            // 자동 재연결
            if config.auto_reconnect && reconnect_attempts < config.max_reconnect_attempts {
                reconnect_attempts += 1;
                *state.write().await = ConnectionState::Reconnecting;
                tokio::time::sleep(Duration::from_millis(config.reconnect_interval_ms)).await;
            } else {
                *state.write().await = ConnectionState::Closed;
                return;
            }
        }
    }

    /// 메시지 전송
    pub fn send(&self, message: &str) -> CcxtResult<()> {
        if let Some(tx) = &self.command_tx {
            tx.send(WsCommand::Send(message.to_string()))
                .map_err(|_| CcxtError::NetworkError {
                    url: self.config.url.clone(),
                    message: "Failed to send message".into(),
                })?;
        }
        Ok(())
    }

    /// 구독 추가
    pub async fn subscribe(&self, subscription: Subscription) -> CcxtResult<()> {
        let key = format!(
            "{}:{}",
            subscription.channel,
            subscription.symbol.as_deref().unwrap_or("")
        );

        // 구독 메시지 전송
        self.send(&subscription.subscribe_message)?;

        // 구독 저장
        self.subscriptions.write().await.insert(key, subscription);

        Ok(())
    }

    /// 구독 해제
    pub async fn unsubscribe(&self, channel: &str, symbol: Option<&str>) -> CcxtResult<()> {
        let key = format!("{}:{}", channel, symbol.unwrap_or(""));

        if let Some(sub) = self.subscriptions.write().await.remove(&key) {
            if let Some(unsub_msg) = sub.unsubscribe_message {
                self.send(&unsub_msg)?;
            }
        }

        Ok(())
    }

    /// 연결 종료
    pub fn close(&self) -> CcxtResult<()> {
        if let Some(tx) = &self.command_tx {
            tx.send(WsCommand::Close).map_err(|_| CcxtError::NetworkError {
                url: self.config.url.clone(),
                message: "Failed to close connection".into(),
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_config_default() {
        let config = WsConfig::default();
        assert!(config.auto_reconnect);
        assert_eq!(config.reconnect_interval_ms, 5000);
        assert_eq!(config.max_reconnect_attempts, 10);
    }

    #[test]
    fn test_ws_client_creation() {
        let client = WsClient::with_url("wss://example.com/ws");
        assert_eq!(client.config.url, "wss://example.com/ws");
    }

    #[tokio::test]
    async fn test_connection_state() {
        let client = WsClient::with_url("wss://example.com/ws");
        assert_eq!(client.state().await, ConnectionState::Disconnected);
    }
}
