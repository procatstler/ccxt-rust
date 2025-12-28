//! Bithumb WebSocket Implementation
//!
//! Bithumb 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://pubwss.bithumb.com/pub/ws";

/// Bithumb WebSocket 클라이언트
pub struct BithumbWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BithumbWs {
    /// 새 Bithumb WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Bithumb 형식으로 변환 (BTC/KRW -> BTC_KRW)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Bithumb 심볼을 통합 심볼로 변환 (BTC_KRW -> BTC/KRW)
    fn to_unified_symbol(bithumb_symbol: &str) -> String {
        bithumb_symbol.replace("_", "/")
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BithumbTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.date.as_ref()
            .and_then(|d| d.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let close_price: Option<Decimal> = data.close_price.as_ref().and_then(|v| v.parse().ok());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_price.as_ref().and_then(|v| v.parse().ok()),
            bid: data.buy_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.sell_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.open_price.as_ref().and_then(|v| v.parse().ok()),
            close: close_price,
            last: close_price,
            previous_close: data.prev_close_price.as_ref().and_then(|v| v.parse().ok()),
            change: data.chg_amt.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.chg_rate.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.value.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BithumbOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.datetime.as_ref()
            .and_then(|d| d.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.list.iter()
            .filter(|item| item.order_type.as_deref() == Some("bid"))
            .filter_map(|item| {
                Some(OrderBookEntry {
                    price: item.price.as_ref()?.parse().ok()?,
                    amount: item.quantity.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.list.iter()
            .filter(|item| item.order_type.as_deref() == Some("ask"))
            .filter_map(|item| {
                Some(OrderBookEntry {
                    price: item.price.as_ref()?.parse().ok()?,
                    amount: item.quantity.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,  // Bithumb sends snapshots
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BithumbTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.cont_date_time.as_ref()
            .and_then(|d| d.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.cont_price.as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data.cont_qty.as_ref()
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();

        let side = match data.buy_sell_gb.as_deref() {
            Some("1") => "sell",  // 매도
            Some("2") => "buy",   // 매수
            _ => "buy",
        };

        let trades = vec![Trade {
            id: data.cont_no.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<BithumbWsResponse>(msg) {
            let symbol = subscribed_symbol.unwrap_or("");

            // Status message (subscribe confirmation)
            if response.status.as_deref() == Some("0000") {
                return Some(WsMessage::Subscribed {
                    channel: response.resmsg.clone().unwrap_or_default(),
                    symbol: Some(symbol.to_string()),
                });
            }

            // Data message
            if let (Some(msg_type), Some(content)) = (&response.msg_type, &response.content) {
                match msg_type.as_str() {
                    "ticker" => {
                        if let Ok(ticker_data) = serde_json::from_value::<BithumbTickerData>(content.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, symbol)));
                        }
                    }
                    "orderbookdepth" => {
                        if let Ok(ob_data) = serde_json::from_value::<BithumbOrderBookData>(content.clone()) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, symbol)));
                        }
                    }
                    "transaction" => {
                        if let Ok(trade_data) = serde_json::from_value::<BithumbTradeWrapper>(content.clone()) {
                            if let Some(trade) = trade_data.list.first() {
                                return Some(WsMessage::Trade(Self::parse_trade(trade, symbol)));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        msg_type: &str,
        symbols: Vec<String>,
        channel: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!({
            "type": msg_type,
            "symbols": symbols,
            "tickTypes": ["30M"]  // 30분 단위 (Bithumb 기본)
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbols.join(","));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let subscribed_symbol = symbols.first().cloned();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    }
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg, subscribed_symbol.as_deref()) {
                            let _ = tx.send(ws_msg);
                        }
                    }
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for BithumbWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BithumbWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let symbols = vec![Self::format_symbol(symbol)];
        client.subscribe_stream("ticker", symbols, "ticker").await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client.subscribe_stream("ticker", formatted, "tickers").await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let symbols = vec![Self::format_symbol(symbol)];
        client.subscribe_stream("orderbookdepth", symbols, "orderBook").await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let symbols = vec![Self::format_symbol(symbol)];
        client.subscribe_stream("transaction", symbols, "trades").await
    }

    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bithumb does not support OHLCV streaming via WebSocket
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_ohlcv".to_string(),
        })
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = &self.ws_client {
            client.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = &self.ws_client {
            client.is_connected().await
        } else {
            false
        }
    }
}

// === Bithumb WebSocket Types ===

#[derive(Debug, Deserialize)]
struct BithumbWsResponse {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    resmsg: Option<String>,
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    content: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbTickerData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    open_price: Option<String>,
    #[serde(default)]
    close_price: Option<String>,
    #[serde(default)]
    high_price: Option<String>,
    #[serde(default)]
    low_price: Option<String>,
    #[serde(default)]
    buy_price: Option<String>,
    #[serde(default)]
    sell_price: Option<String>,
    #[serde(default)]
    prev_close_price: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    chg_amt: Option<String>,
    #[serde(default)]
    chg_rate: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbOrderBookData {
    #[serde(default)]
    datetime: Option<String>,
    #[serde(default)]
    list: Vec<BithumbOrderBookItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbOrderBookItem {
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbTradeWrapper {
    #[serde(default)]
    list: Vec<BithumbTradeData>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BithumbTradeData {
    #[serde(default)]
    cont_no: Option<String>,
    #[serde(default)]
    cont_price: Option<String>,
    #[serde(default)]
    cont_qty: Option<String>,
    #[serde(default)]
    buy_sell_gb: Option<String>,
    #[serde(default)]
    cont_date_time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BithumbWs::format_symbol("BTC/KRW"), "BTC_KRW");
        assert_eq!(BithumbWs::format_symbol("ETH/KRW"), "ETH_KRW");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BithumbWs::to_unified_symbol("BTC_KRW"), "BTC/KRW");
        assert_eq!(BithumbWs::to_unified_symbol("ETH_KRW"), "ETH/KRW");
    }
}
