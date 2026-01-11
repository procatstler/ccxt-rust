//! WebSocket Client
//!
//! WebSocket 연결 관리 및 메시지 처리
//! - 지수 백오프를 사용한 자동 재연결
//! - 연결 상태 모니터링 및 헬스체크
//! - 구독 복원 및 연결 품질 메트릭

use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::errors::{CcxtError, CcxtResult};

/// WebSocket 이벤트
#[derive(Debug, Clone)]
pub enum WsEvent {
    /// 연결됨
    Connected,
    /// 연결 해제됨
    Disconnected,
    /// 재연결 시도 중
    Reconnecting {
        /// 현재 시도 횟수
        attempt: u32,
        /// 최대 시도 횟수
        max_attempts: u32,
        /// 다음 재연결까지 대기 시간 (밀리초)
        delay_ms: u64,
    },
    /// 재연결 성공
    Reconnected {
        /// 시도 횟수
        attempts: u32,
    },
    /// 메시지 수신
    Message(String),
    /// 에러 발생
    Error(String),
    /// Ping 수신
    Ping,
    /// Pong 수신
    Pong,
    /// 연결 상태 정상
    HealthOk,
    /// 연결 상태 이상 (heartbeat 누락)
    HealthWarning {
        /// 마지막 pong 이후 경과 시간 (초)
        last_pong_ago_secs: u64,
    },
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

/// 재연결 설정
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// 자동 재연결 여부
    pub enabled: bool,
    /// 초기 재연결 대기 시간 (밀리초)
    pub initial_delay_ms: u64,
    /// 최대 재연결 대기 시간 (밀리초)
    pub max_delay_ms: u64,
    /// 백오프 배수
    pub backoff_multiplier: f64,
    /// 최대 재연결 시도 횟수 (0 = 무제한)
    pub max_attempts: u32,
    /// 지터 범위 (0.0 ~ 1.0)
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
    /// 지수 백오프로 다음 재연결 대기 시간 계산
    pub fn calculate_delay(&self, attempt: u32) -> u64 {
        let base_delay =
            self.initial_delay_ms as f64 * self.backoff_multiplier.powi(attempt as i32);
        let delay = base_delay.min(self.max_delay_ms as f64);

        // 지터 추가 (동시 재연결 방지)
        if self.jitter > 0.0 {
            let jitter_range = delay * self.jitter;
            let jitter = (rand_simple() * jitter_range * 2.0) - jitter_range;
            (delay + jitter).max(0.0) as u64
        } else {
            delay as u64
        }
    }
}

/// 헬스체크 설정
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// 헬스체크 활성화 여부
    pub enabled: bool,
    /// Ping 전송 간격 (초)
    pub ping_interval_secs: u64,
    /// Pong 응답 타임아웃 (초)
    pub pong_timeout_secs: u64,
    /// 허용되는 최대 연속 ping 미응답 횟수
    pub max_missed_pongs: u32,
    /// 헬스체크 경고 임계값 (초)
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

/// WebSocket 클라이언트 설정
#[derive(Debug, Clone)]
pub struct WsConfig {
    /// WebSocket URL
    pub url: String,
    /// 연결 타임아웃 (초)
    pub connect_timeout_secs: u64,
    /// 재연결 설정
    pub reconnect: ReconnectConfig,
    /// 헬스체크 설정
    pub health_check: HealthCheckConfig,
    /// 구독 복원 시 대기 시간 (밀리초)
    pub subscription_delay_ms: u64,
    /// 메시지 큐 크기
    pub message_buffer_size: usize,

    // 하위 호환성을 위한 레거시 필드
    /// 자동 재연결 여부 (레거시)
    pub auto_reconnect: bool,
    /// 재연결 간격 (레거시)
    pub reconnect_interval_ms: u64,
    /// 최대 재연결 시도 횟수 (레거시)
    pub max_reconnect_attempts: u32,
    /// Ping 간격 (레거시)
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
            // 레거시 필드
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
        }
    }
}

impl WsConfig {
    /// 재연결 설정 적용
    pub fn with_reconnect(mut self, config: ReconnectConfig) -> Self {
        self.reconnect = config;
        self
    }

    /// 헬스체크 설정 적용
    pub fn with_health_check(mut self, config: HealthCheckConfig) -> Self {
        self.health_check = config;
        self
    }

    /// 재연결 비활성화
    pub fn without_reconnect(mut self) -> Self {
        self.reconnect.enabled = false;
        self.auto_reconnect = false;
        self
    }
}

/// WebSocket 연결 메트릭
#[derive(Debug, Default)]
pub struct WsMetrics {
    /// 총 연결 횟수
    pub total_connections: AtomicUsize,
    /// 총 재연결 횟수
    pub total_reconnections: AtomicUsize,
    /// 연속 재연결 실패 횟수
    pub consecutive_failures: AtomicUsize,
    /// 총 전송 메시지 수
    pub messages_sent: AtomicU64,
    /// 총 수신 메시지 수
    pub messages_received: AtomicU64,
    /// 마지막 연결 시간 (Unix timestamp ms)
    pub last_connected_at: AtomicU64,
    /// 마지막 pong 수신 시간 (Unix timestamp ms)
    pub last_pong_at: AtomicU64,
    /// 마지막 메시지 수신 시간 (Unix timestamp ms)
    pub last_message_at: AtomicU64,
}

impl WsMetrics {
    /// 새 메트릭 인스턴스 생성
    pub fn new() -> Self {
        Self::default()
    }

    /// 연결 기록
    pub fn record_connection(&self) {
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.last_connected_at.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            Ordering::Relaxed,
        );
    }

    /// 재연결 기록
    pub fn record_reconnection(&self) {
        self.total_reconnections.fetch_add(1, Ordering::Relaxed);
        self.record_connection();
    }

    /// 연결 실패 기록
    pub fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// 메시지 전송 기록
    pub fn record_message_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// 메시지 수신 기록
    pub fn record_message_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        self.last_message_at.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            Ordering::Relaxed,
        );
    }

    /// Pong 수신 기록
    pub fn record_pong(&self) {
        self.last_pong_at.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            Ordering::Relaxed,
        );
    }

    /// 마지막 pong 이후 경과 시간 (초)
    pub fn seconds_since_last_pong(&self) -> u64 {
        let last_pong = self.last_pong_at.load(Ordering::Relaxed);
        if last_pong == 0 {
            return u64::MAX;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        (now.saturating_sub(last_pong)) / 1000
    }

    /// 스냅샷 조회
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

/// WebSocket 메트릭 스냅샷
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

/// 간단한 난수 생성 (지터용)
fn rand_simple() -> f64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    hasher.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64,
    );
    (hasher.finish() % 1000) as f64 / 1000.0
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
    metrics: Arc<WsMetrics>,
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
            metrics: Arc::new(WsMetrics::new()),
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

    /// 메트릭 조회
    pub fn metrics(&self) -> &WsMetrics {
        &self.metrics
    }

    /// 메트릭 스냅샷 조회
    pub fn metrics_snapshot(&self) -> WsMetricsSnapshot {
        self.metrics.snapshot()
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
        let metrics = self.metrics.clone();

        // 연결 태스크 시작
        tokio::spawn(async move {
            Self::connection_loop(config, state, subscriptions, event_tx, command_rx, metrics)
                .await;
        });

        Ok(event_rx)
    }

    /// 연결 루프 (개선된 재연결 및 헬스체크 포함)
    async fn connection_loop(
        config: WsConfig,
        state: Arc<RwLock<ConnectionState>>,
        subscriptions: Arc<RwLock<HashMap<String, Subscription>>>,
        event_tx: mpsc::UnboundedSender<WsEvent>,
        mut command_rx: mpsc::UnboundedReceiver<WsCommand>,
        metrics: Arc<WsMetrics>,
    ) {
        let mut reconnect_attempts: u32 = 0;
        let mut is_first_connection = true;
        #[allow(unused_assignments)]
        let mut missed_pongs: u32 = 0;

        // 재연결 활성화 여부 (레거시 호환)
        let reconnect_enabled = config.reconnect.enabled || config.auto_reconnect;
        let max_attempts = if config.reconnect.max_attempts > 0 {
            config.reconnect.max_attempts
        } else {
            config.max_reconnect_attempts
        };

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
                    // 연결 성공
                    if is_first_connection {
                        metrics.record_connection();
                        let _ = event_tx.send(WsEvent::Connected);
                    } else {
                        metrics.record_reconnection();
                        let _ = event_tx.send(WsEvent::Reconnected {
                            attempts: reconnect_attempts,
                        });
                    }

                    reconnect_attempts = 0;
                    missed_pongs = 0;
                    is_first_connection = false;
                    *state.write().await = ConnectionState::Connected;
                    (stream, response)
                },
                Ok(Err(e)) => {
                    metrics.record_failure();
                    let _ = event_tx.send(WsEvent::Error(format!("Connection error: {e}")));

                    if reconnect_enabled && (max_attempts == 0 || reconnect_attempts < max_attempts)
                    {
                        let delay = config.reconnect.calculate_delay(reconnect_attempts);
                        reconnect_attempts += 1;

                        let _ = event_tx.send(WsEvent::Reconnecting {
                            attempt: reconnect_attempts,
                            max_attempts,
                            delay_ms: delay,
                        });

                        *state.write().await = ConnectionState::Reconnecting;
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    } else {
                        *state.write().await = ConnectionState::Closed;
                        return;
                    }
                },
                Err(_) => {
                    metrics.record_failure();
                    let _ = event_tx.send(WsEvent::Error("Connection timeout".into()));

                    if reconnect_enabled && (max_attempts == 0 || reconnect_attempts < max_attempts)
                    {
                        let delay = config.reconnect.calculate_delay(reconnect_attempts);
                        reconnect_attempts += 1;

                        let _ = event_tx.send(WsEvent::Reconnecting {
                            attempt: reconnect_attempts,
                            max_attempts,
                            delay_ms: delay,
                        });

                        *state.write().await = ConnectionState::Reconnecting;
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    } else {
                        *state.write().await = ConnectionState::Closed;
                        return;
                    }
                },
            };

            let (mut write, mut read) = ws_stream.split();

            // 초기 pong 시간 기록 (연결 직후)
            metrics.record_pong();

            // 기존 구독 복원 (지연 적용)
            {
                let subs = subscriptions.read().await;
                for (_, sub) in subs.iter() {
                    let _ = write
                        .send(Message::Text(sub.subscribe_message.clone()))
                        .await;
                    metrics.record_message_sent();

                    // 구독 간 지연 (서버 과부하 방지)
                    if config.subscription_delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(config.subscription_delay_ms))
                            .await;
                    }
                }
            }

            // Ping 타이머
            let ping_interval_secs = if config.health_check.enabled {
                config.health_check.ping_interval_secs
            } else {
                config.ping_interval_secs
            };
            let mut ping_interval = interval(Duration::from_secs(ping_interval_secs));

            // 헬스체크 타이머
            let mut health_check_interval = interval(Duration::from_secs(
                config.health_check.warning_threshold_secs.max(10),
            ));

            loop {
                tokio::select! {
                    // WebSocket 메시지 수신
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                metrics.record_message_received();
                                let _ = event_tx.send(WsEvent::Message(text));
                            }
                            Some(Ok(Message::Binary(data))) => {
                                metrics.record_message_received();
                                if let Ok(text) = String::from_utf8(data) {
                                    let _ = event_tx.send(WsEvent::Message(text));
                                }
                            }
                            Some(Ok(Message::Ping(data))) => {
                                let _ = event_tx.send(WsEvent::Ping);
                                let _ = write.send(Message::Pong(data)).await;
                            }
                            Some(Ok(Message::Pong(_))) => {
                                metrics.record_pong();
                                missed_pongs = 0;
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
                                if write.send(Message::Text(msg)).await.is_ok() {
                                    metrics.record_message_sent();
                                } else {
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
                        if config.health_check.enabled {
                            // Pong 미수신 추적
                            missed_pongs += 1;

                            if missed_pongs >= config.health_check.max_missed_pongs {
                                let _ = event_tx.send(WsEvent::Error(
                                    format!("Health check failed: {missed_pongs} missed pongs")
                                ));
                                break;
                            }
                        }

                        if write.send(Message::Ping(vec![])).await.is_err() {
                            break;
                        }
                    }

                    // 헬스체크 타이머
                    _ = health_check_interval.tick() => {
                        if config.health_check.enabled {
                            let secs_since_pong = metrics.seconds_since_last_pong();

                            if secs_since_pong > config.health_check.warning_threshold_secs
                                && secs_since_pong < u64::MAX
                            {
                                let _ = event_tx.send(WsEvent::HealthWarning {
                                    last_pong_ago_secs: secs_since_pong,
                                });
                            } else {
                                let _ = event_tx.send(WsEvent::HealthOk);
                            }
                        }
                    }
                }
            }

            // 연결 해제됨
            *state.write().await = ConnectionState::Disconnected;

            // 자동 재연결 (지수 백오프 적용)
            if reconnect_enabled && (max_attempts == 0 || reconnect_attempts < max_attempts) {
                let delay = config.reconnect.calculate_delay(reconnect_attempts);
                reconnect_attempts += 1;

                let _ = event_tx.send(WsEvent::Reconnecting {
                    attempt: reconnect_attempts,
                    max_attempts,
                    delay_ms: delay,
                });

                *state.write().await = ConnectionState::Reconnecting;
                tokio::time::sleep(Duration::from_millis(delay)).await;
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
            tx.send(WsCommand::Close)
                .map_err(|_| CcxtError::NetworkError {
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
