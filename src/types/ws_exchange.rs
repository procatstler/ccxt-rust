//! WebSocket Exchange trait
//!
//! 실시간 데이터 스트리밍을 위한 WebSocket 거래소 인터페이스

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::errors::CcxtResult;
use super::{OrderBook, Position, Ticker, Trade, OHLCV, Order, Balances, Timeframe};

/// WebSocket 티커 이벤트
#[derive(Debug, Clone)]
pub struct WsTickerEvent {
    pub symbol: String,
    pub ticker: Ticker,
}

/// WebSocket 호가창 이벤트
#[derive(Debug, Clone)]
pub struct WsOrderBookEvent {
    pub symbol: String,
    pub order_book: OrderBook,
    /// 스냅샷 여부 (false면 delta)
    pub is_snapshot: bool,
}

/// WebSocket 체결 이벤트
#[derive(Debug, Clone)]
pub struct WsTradeEvent {
    pub symbol: String,
    pub trades: Vec<Trade>,
}

/// WebSocket OHLCV 이벤트
#[derive(Debug, Clone)]
pub struct WsOhlcvEvent {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub ohlcv: OHLCV,
}

/// WebSocket 주문 이벤트
#[derive(Debug, Clone)]
pub struct WsOrderEvent {
    pub order: Order,
}

/// WebSocket 잔고 이벤트
#[derive(Debug, Clone)]
pub struct WsBalanceEvent {
    pub balances: Balances,
}

/// WebSocket 포지션 이벤트
#[derive(Debug, Clone)]
pub struct WsPositionEvent {
    pub positions: Vec<Position>,
}

/// WebSocket 내 체결 이벤트 (Private)
#[derive(Debug, Clone)]
pub struct WsMyTradeEvent {
    pub symbol: String,
    pub trades: Vec<Trade>,
}

/// WebSocket 스트림 메시지 타입
#[derive(Debug, Clone)]
pub enum WsMessage {
    /// 티커 업데이트
    Ticker(WsTickerEvent),
    /// 호가창 업데이트
    OrderBook(WsOrderBookEvent),
    /// 체결 내역
    Trade(WsTradeEvent),
    /// OHLCV 캔들
    Ohlcv(WsOhlcvEvent),
    /// 주문 업데이트 (비공개)
    Order(WsOrderEvent),
    /// 잔고 업데이트 (비공개)
    Balance(WsBalanceEvent),
    /// 포지션 업데이트 (비공개)
    Position(WsPositionEvent),
    /// 내 체결 업데이트 (비공개)
    MyTrade(WsMyTradeEvent),
    /// 연결됨
    Connected,
    /// 연결 해제됨
    Disconnected,
    /// 에러
    Error(String),
    /// 인증 성공
    Authenticated,
    /// 구독 완료
    Subscribed { channel: String, symbol: Option<String> },
    /// 구독 해제 완료
    Unsubscribed { channel: String, symbol: Option<String> },
}

/// WebSocket 거래소 인터페이스
#[async_trait]
pub trait WsExchange: Send + Sync {
    // === Public Streams ===

    /// 티커 구독
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>>;

    /// 복수 티커 구독
    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchTickers".into(),
        })
    }

    /// 호가창 구독
    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>>;

    /// 복수 호가창 구독
    async fn watch_order_book_for_symbols(&self, symbols: &[&str], limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = (symbols, limit);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOrderBookForSymbols".into(),
        })
    }

    /// 체결 내역 구독
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>>;

    /// 복수 체결 내역 구독
    async fn watch_trades_for_symbols(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchTradesForSymbols".into(),
        })
    }

    /// OHLCV 구독
    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>>;

    /// 복수 OHLCV 구독
    async fn watch_ohlcv_for_symbols(&self, symbols: &[&str], timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = (symbols, timeframe);
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOhlcvForSymbols".into(),
        })
    }

    // === Futures/Derivatives Streams ===

    /// 마크 가격 구독 (선물/무기한)
    async fn watch_mark_price(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchMarkPrice".into(),
        })
    }

    /// 복수 마크 가격 구독
    async fn watch_mark_prices(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchMarkPrices".into(),
        })
    }

    /// 포지션 구독 (인증 필요)
    async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbols;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchPositions".into(),
        })
    }

    // === Private Streams ===

    /// 잔고 변경 구독 (인증 필요)
    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchBalance".into(),
        })
    }

    /// 주문 변경 구독 (인증 필요)
    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOrders".into(),
        })
    }

    /// 내 체결 내역 구독 (인증 필요)
    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let _ = symbol;
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchMyTrades".into(),
        })
    }

    // === Connection Management ===

    /// WebSocket 연결
    async fn ws_connect(&mut self) -> CcxtResult<()>;

    /// WebSocket 연결 종료
    async fn ws_close(&mut self) -> CcxtResult<()>;

    /// WebSocket 연결 상태 확인
    async fn ws_is_connected(&self) -> bool;

    /// WebSocket 인증 (비공개 스트림용)
    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "wsAuthenticate".into(),
        })
    }
}

/// WebSocket 구독 핸들
/// 구독을 관리하고 드롭시 자동 구독 해제
pub struct WsSubscription {
    channel: String,
    symbol: Option<String>,
    unsubscribe_fn: Option<Box<dyn FnOnce() + Send>>,
}

impl WsSubscription {
    /// 새 구독 핸들 생성
    pub fn new(channel: String, symbol: Option<String>) -> Self {
        Self {
            channel,
            symbol,
            unsubscribe_fn: None,
        }
    }

    /// 구독 해제 함수 설정
    pub fn with_unsubscribe<F: FnOnce() + Send + 'static>(mut self, f: F) -> Self {
        self.unsubscribe_fn = Some(Box::new(f));
        self
    }

    /// 채널 이름 반환
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// 심볼 반환
    pub fn symbol(&self) -> Option<&str> {
        self.symbol.as_deref()
    }
}

impl Drop for WsSubscription {
    fn drop(&mut self) {
        if let Some(f) = self.unsubscribe_fn.take() {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_message_types() {
        let msg = WsMessage::Connected;
        assert!(matches!(msg, WsMessage::Connected));

        let msg = WsMessage::Error("test error".into());
        if let WsMessage::Error(e) = msg {
            assert_eq!(e, "test error");
        } else {
            panic!("Expected Error variant");
        }
    }

    #[test]
    fn test_ws_subscription() {
        let sub = WsSubscription::new("ticker".into(), Some("BTC/USDT".into()));
        assert_eq!(sub.channel(), "ticker");
        assert_eq!(sub.symbol(), Some("BTC/USDT"));
    }
}
