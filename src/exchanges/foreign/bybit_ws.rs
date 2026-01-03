//! Bybit WebSocket Implementation
//!
//! Bybit 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balances, Balance, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Position, PositionSide, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, OHLCV,
    WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent, WsOhlcvEvent, WsOrderBookEvent,
    WsOrderEvent, WsPositionEvent, WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://stream.bybit.com/v5/public/spot";
const WS_LINEAR_URL: &str = "wss://stream.bybit.com/v5/public/linear";
const WS_PRIVATE_URL: &str = "wss://stream.bybit.com/v5/private";

type HmacSha256 = Hmac<Sha256>;

/// Bybit WebSocket 클라이언트
pub struct BybitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    /// API key for private channels
    api_key: Option<String>,
    /// API secret for private channels
    api_secret: Option<String>,
}

impl BybitWs {
    /// 새 Bybit WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
        }
    }

    /// API 키와 함께 생성 (Private 채널용)
    pub fn with_credentials(api_key: &str, api_secret: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
        }
    }

    /// API 자격증명 설정
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
    }

    /// API 키 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// WebSocket 인증 서명 생성
    fn generate_auth_signature(api_secret: &str, expires: i64) -> String {
        let sign_str = format!("GET/realtime{}", expires);
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_str.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// 심볼을 Bybit 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_uppercase()
    }

    /// Timeframe을 Bybit interval로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1",
            Timeframe::Minute3 => "3",
            Timeframe::Minute5 => "5",
            Timeframe::Minute15 => "15",
            Timeframe::Minute30 => "30",
            Timeframe::Hour1 => "60",
            Timeframe::Hour2 => "120",
            Timeframe::Hour4 => "240",
            Timeframe::Hour6 => "360",
            Timeframe::Hour12 => "720",
            Timeframe::Day1 => "D",
            Timeframe::Week1 => "W",
            Timeframe::Month1 => "M",
            _ => "1",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BybitTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price_24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_price_24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid1_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid1_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask1_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask1_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: None,
            close: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: data.prev_price_24h.as_ref().and_then(|v| v.parse().ok()),
            change: data.price_24h_pcnt.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.price_24h_pcnt.as_ref().and_then(|v| v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))),
            average: None,
            base_volume: data.volume_24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.turnover_24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BybitOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.b.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.a.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.u,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BybitTradeData, symbol: &str) -> WsTradeEvent {
        let trades: Vec<Trade> = vec![{
            let timestamp = data.T.unwrap_or_else(|| Utc::now().timestamp_millis());
            let price: Decimal = data.p.parse().unwrap_or_default();
            let amount: Decimal = data.v.parse().unwrap_or_default();

            Trade {
                id: data.i.clone(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(data.S.to_lowercase()),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(data).unwrap_or_default(),
            }
        }];

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades,
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_kline(data: &BybitKlineData, timeframe: Timeframe) -> WsOhlcvEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.start;

        let ohlcv = OHLCV::new(
            timestamp,
            data.open.parse().unwrap_or_default(),
            data.high.parse().unwrap_or_default(),
            data.low.parse().unwrap_or_default(),
            data.close.parse().unwrap_or_default(),
            data.volume.parse().unwrap_or_default(),
        );

        WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }
    }

    /// Bybit 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(bybit_symbol: &str) -> String {
        let quotes = ["USDT", "USDC", "BTC", "ETH", "EUR", "DAI"];

        for quote in quotes {
            if let Some(base) = bybit_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }

        bybit_symbol.to_string()
    }

    // === Private Channel Parsers ===

    /// 주문 업데이트 파싱
    fn parse_order(data: &BybitOrderData) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.updated_time.parse::<i64>().unwrap_or_else(|_| Utc::now().timestamp_millis());

        let status = match data.order_status.as_str() {
            "New" | "Created" => OrderStatus::Open,
            "PartiallyFilled" => OrderStatus::Open,
            "Filled" => OrderStatus::Closed,
            "Cancelled" | "Canceled" => OrderStatus::Canceled,
            "Rejected" => OrderStatus::Rejected,
            "PartiallyFilledCanceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "Limit" => OrderType::Limit,
            "Market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = if data.side == "Buy" {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        };

        let time_in_force = match data.time_in_force.as_deref() {
            Some("GTC") => Some(TimeInForce::GTC),
            Some("IOC") => Some(TimeInForce::IOC),
            Some("FOK") => Some(TimeInForce::FOK),
            Some("PostOnly") => Some(TimeInForce::PO),
            _ => None,
        };

        let order = Order {
            id: data.order_id.clone(),
            client_order_id: data.order_link_id.clone(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force,
            side,
            price: data.price.parse().ok(),
            average: data.avg_price.as_ref().and_then(|p| p.parse().ok()),
            amount: data.qty.parse().unwrap_or_default(),
            filled: data.cum_exec_qty.as_ref().and_then(|q| q.parse().ok()).unwrap_or_default(),
            remaining: data.leaves_qty.as_ref().and_then(|q| q.parse().ok()),
            stop_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            take_profit_price: data.take_profit.as_ref().and_then(|p| p.parse().ok()),
            stop_loss_price: data.stop_loss.as_ref().and_then(|p| p.parse().ok()),
            cost: data.cum_exec_value.as_ref().and_then(|v| v.parse().ok()),
            trades: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: Some(data.time_in_force.as_deref() == Some("PostOnly")),
            fee: data.cum_exec_fee.as_ref().and_then(|f| f.parse().ok()).map(|cost| Fee {
                currency: None,
                cost: Some(cost),
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 체결 내역 파싱 (execution)
    fn parse_execution(data: &BybitExecutionData) -> WsMyTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.exec_time.parse::<i64>().unwrap_or_else(|_| Utc::now().timestamp_millis());
        let price: Decimal = data.exec_price.parse().unwrap_or_default();
        let amount: Decimal = data.exec_qty.parse().unwrap_or_default();

        let taker_or_maker = if data.is_maker.unwrap_or(false) {
            Some(TakerOrMaker::Maker)
        } else {
            Some(TakerOrMaker::Taker)
        };

        let trade = Trade {
            id: data.exec_id.clone(),
            order: Some(data.order_id.clone()),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.side.to_lowercase()),
            taker_or_maker,
            price,
            amount,
            cost: Some(price * amount),
            fee: data.exec_fee.as_ref().and_then(|f| f.parse().ok()).map(|cost| Fee {
                currency: data.fee_currency.clone(),
                cost: Some(cost),
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsMyTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// 포지션 업데이트 파싱
    fn parse_position(data: &BybitPositionData) -> WsPositionEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.updated_time.parse::<i64>().unwrap_or_else(|_| Utc::now().timestamp_millis());

        let side = match data.side.as_str() {
            "Buy" => Some(PositionSide::Long),
            "Sell" => Some(PositionSide::Short),
            _ => None,
        };

        let margin_mode = match data.trade_mode.as_deref() {
            Some("0") => Some(MarginMode::Cross),
            Some("1") => Some(MarginMode::Isolated),
            _ => Some(MarginMode::Cross),
        };

        let position = Position {
            id: None,
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            side,
            contracts: data.size.as_ref().and_then(|s| s.parse().ok()),
            contract_size: None,
            entry_price: data.entry_price.as_ref().and_then(|p| p.parse().ok()),
            notional: data.position_value.as_ref().and_then(|v| v.parse().ok()),
            leverage: data.leverage.as_ref().and_then(|l| l.parse().ok()),
            unrealized_pnl: data.unrealised_pnl.as_ref().and_then(|p| p.parse().ok()),
            realized_pnl: data.cum_realised_pnl.as_ref().and_then(|p| p.parse().ok()),
            collateral: data.position_im.as_ref().and_then(|c| c.parse().ok()),
            initial_margin: data.position_im.as_ref().and_then(|m| m.parse().ok()),
            initial_margin_percentage: None,
            maintenance_margin: data.position_mm.as_ref().and_then(|m| m.parse().ok()),
            maintenance_margin_percentage: None,
            margin_ratio: None,
            margin_mode,
            liquidation_price: data.liq_price.as_ref().and_then(|p| p.parse().ok()),
            mark_price: data.mark_price.as_ref().and_then(|p| p.parse().ok()),
            last_price: None,
            last_update_timestamp: Some(timestamp),
            hedged: None,
            percentage: None,
            stop_loss_price: data.stop_loss.as_ref().and_then(|p| p.parse().ok()),
            take_profit_price: data.take_profit.as_ref().and_then(|p| p.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsPositionEvent {
            positions: vec![position],
        }
    }

    /// 잔고 업데이트 파싱
    fn parse_wallet(data: &BybitWalletData) -> WsBalanceEvent {
        let timestamp = Utc::now().timestamp_millis();
        let mut currencies = HashMap::new();

        for coin in &data.coin {
            let currency = coin.coin.clone();
            let free: Decimal = coin.available_to_borrow.as_ref()
                .or(coin.available_to_withdraw.as_ref())
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();
            let total: Decimal = coin.wallet_balance.as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();
            let used = total - free;

            currencies.insert(currency, Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            });
        }

        let balances = Balances {
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsBalanceEvent { balances }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Bybit 메시지 파싱
        if let Ok(response) = serde_json::from_str::<BybitWsResponse>(msg) {
            // Authentication response (handle before generic error check)
            if response.op == Some("auth".to_string()) {
                if response.ret_code == Some(0) {
                    return Some(WsMessage::Authenticated);
                } else {
                    return Some(WsMessage::Error(format!(
                        "Authentication failed: {}",
                        response.ret_msg.unwrap_or_default()
                    )));
                }
            }

            // 에러 체크
            if response.ret_code.is_some() && response.ret_code != Some(0) {
                return Some(WsMessage::Error(response.ret_msg.unwrap_or_default()));
            }

            // 구독 확인
            if response.op == Some("subscribe".to_string()) {
                return Some(WsMessage::Subscribed {
                    channel: "".to_string(),
                    symbol: None,
                });
            }

            // 데이터 처리
            if let Some(topic) = &response.topic {
                if let Some(data) = &response.data {
                    // Ticker
                    if topic.starts_with("tickers.") {
                        if let Ok(ticker_data) = serde_json::from_value::<BybitTickerData>(data.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }

                    // OrderBook
                    if topic.starts_with("orderbook.") {
                        if let Ok(ob_data) = serde_json::from_value::<BybitOrderBookData>(data.clone()) {
                            let symbol = Self::to_unified_symbol(&ob_data.s);
                            let is_snapshot = response.response_type == Some("snapshot".to_string());
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                        }
                    }

                    // Trade
                    if topic.starts_with("publicTrade.") {
                        if let Ok(trades) = serde_json::from_value::<Vec<BybitTradeData>>(data.clone()) {
                            if let Some(trade_data) = trades.first() {
                                let symbol = Self::to_unified_symbol(&trade_data.s);
                                return Some(WsMessage::Trade(Self::parse_trade(trade_data, &symbol)));
                            }
                        }
                    }

                    // Kline
                    if topic.starts_with("kline.") {
                        if let Ok(kline_list) = serde_json::from_value::<Vec<BybitKlineData>>(data.clone()) {
                            if let Some(kline_data) = kline_list.first() {
                                // Extract interval from topic (e.g., "kline.1.BTCUSDT")
                                let parts: Vec<&str> = topic.split('.').collect();
                                let timeframe = if parts.len() >= 2 {
                                    match parts[1] {
                                        "1" => Timeframe::Minute1,
                                        "3" => Timeframe::Minute3,
                                        "5" => Timeframe::Minute5,
                                        "15" => Timeframe::Minute15,
                                        "30" => Timeframe::Minute30,
                                        "60" => Timeframe::Hour1,
                                        "120" => Timeframe::Hour2,
                                        "240" => Timeframe::Hour4,
                                        "360" => Timeframe::Hour6,
                                        "720" => Timeframe::Hour12,
                                        "D" => Timeframe::Day1,
                                        "W" => Timeframe::Week1,
                                        "M" => Timeframe::Month1,
                                        _ => Timeframe::Minute1,
                                    }
                                } else {
                                    Timeframe::Minute1
                                };
                                return Some(WsMessage::Ohlcv(Self::parse_kline(kline_data, timeframe)));
                            }
                        }
                    }

                    // === Private Channels ===

                    // Order updates
                    if topic == "order" {
                        if let Ok(orders) = serde_json::from_value::<Vec<BybitOrderData>>(data.clone()) {
                            if let Some(order_data) = orders.first() {
                                return Some(WsMessage::Order(Self::parse_order(order_data)));
                            }
                        }
                    }

                    // Execution (fills)
                    if topic == "execution" {
                        if let Ok(executions) = serde_json::from_value::<Vec<BybitExecutionData>>(data.clone()) {
                            if let Some(exec_data) = executions.first() {
                                return Some(WsMessage::MyTrade(Self::parse_execution(exec_data)));
                            }
                        }
                    }

                    // Position updates
                    if topic == "position" {
                        if let Ok(positions) = serde_json::from_value::<Vec<BybitPositionData>>(data.clone()) {
                            if let Some(pos_data) = positions.first() {
                                return Some(WsMessage::Position(Self::parse_position(pos_data)));
                            }
                        }
                    }

                    // Wallet updates
                    if topic == "wallet" {
                        if let Ok(wallets) = serde_json::from_value::<Vec<BybitWalletData>>(data.clone()) {
                            if let Some(wallet_data) = wallets.first() {
                                return Some(WsMessage::Balance(Self::parse_wallet(wallet_data)));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, topics: Vec<String>, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": topics
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, topics.join(","));
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
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
                        if let Some(ws_msg) = Self::process_message(&msg) {
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

    /// Private 채널 구독 (인증 필요)
    async fn subscribe_private_stream(
        &mut self,
        topics: Vec<String>,
        channel: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for private channels".to_string(),
        })?;
        let api_secret = self.api_secret.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required for private channels".to_string(),
        })?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket 클라이언트 생성 및 연결 (Private URL)
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 인증 메시지 전송
        let expires = Utc::now().timestamp_millis() + 10000;
        let signature = Self::generate_auth_signature(&api_secret, expires);
        let auth_msg = serde_json::json!({
            "op": "auth",
            "args": [api_key, expires.to_string(), signature]
        });
        ws_client.send(&auth_msg.to_string())?;

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": topics
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:private", channel);
            self.subscriptions.write().await.insert(key, topics.join(","));
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
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
                        if let Some(ws_msg) = Self::process_message(&msg) {
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

impl Default for BybitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BybitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topic = format!("tickers.{}", Self::format_symbol(symbol));
        client.subscribe_stream(vec![topic], "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topics: Vec<String> = symbols
            .iter()
            .map(|s| format!("tickers.{}", Self::format_symbol(s)))
            .collect();
        client.subscribe_stream(topics, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let depth = match limit.unwrap_or(50) {
            1 => 1,
            25 => 25,
            50 => 50,
            100 => 100,
            200 => 200,
            _ => 50,
        };
        let topic = format!("orderbook.{}.{}", depth, Self::format_symbol(symbol));
        client.subscribe_stream(vec![topic], "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topic = format!("publicTrade.{}", Self::format_symbol(symbol));
        client.subscribe_stream(vec![topic], "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let topic = format!("kline.{}.{}", interval, Self::format_symbol(symbol));
        client.subscribe_stream(vec![topic], "ohlcv", Some(symbol)).await
    }

    // === Private Channels ===

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        };
        client.subscribe_private_stream(vec!["order".to_string()], "order").await
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        };
        client.subscribe_private_stream(vec!["execution".to_string()], "execution").await
    }

    async fn watch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        };
        client.subscribe_private_stream(vec!["position".to_string()], "position").await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        };
        client.subscribe_private_stream(vec!["wallet".to_string()], "wallet").await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        let api_key = self.api_key.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".to_string(),
        })?;
        let api_secret = self.api_secret.clone().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".to_string(),
        })?;

        if let Some(client) = &self.ws_client {
            let expires = Utc::now().timestamp_millis() + 10000;
            let signature = Self::generate_auth_signature(&api_secret, expires);
            let auth_msg = serde_json::json!({
                "op": "auth",
                "args": [api_key, expires.to_string(), signature]
            });
            client.send(&auth_msg.to_string())?;
        }
        Ok(())
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

// === Bybit WebSocket Message Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitWsResponse {
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default, rename = "type")]
    response_type: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default, rename = "retCode")]
    ret_code: Option<i32>,
    #[serde(default, rename = "retMsg")]
    ret_msg: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitTickerData {
    symbol: String,
    #[serde(default, rename = "lastPrice")]
    last_price: Option<String>,
    #[serde(default, rename = "highPrice24h")]
    high_price_24h: Option<String>,
    #[serde(default, rename = "lowPrice24h")]
    low_price_24h: Option<String>,
    #[serde(default, rename = "prevPrice24h")]
    prev_price_24h: Option<String>,
    #[serde(default, rename = "volume24h")]
    volume_24h: Option<String>,
    #[serde(default, rename = "turnover24h")]
    turnover_24h: Option<String>,
    #[serde(default, rename = "price24hPcnt")]
    price_24h_pcnt: Option<String>,
    #[serde(default, rename = "bid1Price")]
    bid1_price: Option<String>,
    #[serde(default, rename = "bid1Size")]
    bid1_size: Option<String>,
    #[serde(default, rename = "ask1Price")]
    ask1_price: Option<String>,
    #[serde(default, rename = "ask1Size")]
    ask1_size: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BybitOrderBookData {
    s: String, // Symbol
    #[serde(default)]
    b: Vec<Vec<String>>, // Bids [price, size]
    #[serde(default)]
    a: Vec<Vec<String>>, // Asks [price, size]
    #[serde(default)]
    u: Option<i64>, // Update ID
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BybitTradeData {
    s: String,  // Symbol
    i: String,  // Trade ID
    p: String,  // Price
    v: String,  // Size/Volume
    S: String,  // Side: Buy/Sell
    #[serde(default)]
    T: Option<i64>, // Trade time
    #[serde(default)]
    BT: Option<bool>, // Block trade
}

#[derive(Debug, Deserialize, Serialize)]
struct BybitKlineData {
    symbol: String,
    start: i64,
    end: i64,
    interval: String,
    open: String,
    close: String,
    high: String,
    low: String,
    volume: String,
    turnover: String,
    #[serde(default)]
    confirm: Option<bool>,
}

// === Private Channel Data Types ===

/// 주문 업데이트 데이터
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitOrderData {
    symbol: String,
    order_id: String,
    #[serde(default)]
    order_link_id: Option<String>,
    side: String,
    order_type: String,
    price: String,
    qty: String,
    #[serde(default)]
    time_in_force: Option<String>,
    order_status: String,
    #[serde(default)]
    cum_exec_qty: Option<String>,
    #[serde(default)]
    cum_exec_value: Option<String>,
    #[serde(default)]
    cum_exec_fee: Option<String>,
    #[serde(default)]
    leaves_qty: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    trigger_price: Option<String>,
    #[serde(default)]
    take_profit: Option<String>,
    #[serde(default)]
    stop_loss: Option<String>,
    updated_time: String,
}

/// 체결 데이터
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitExecutionData {
    symbol: String,
    exec_id: String,
    order_id: String,
    #[serde(default)]
    order_link_id: Option<String>,
    side: String,
    exec_price: String,
    exec_qty: String,
    exec_time: String,
    #[serde(default)]
    exec_fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    is_maker: Option<bool>,
}

/// 포지션 데이터
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitPositionData {
    symbol: String,
    side: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    entry_price: Option<String>,
    #[serde(default)]
    leverage: Option<String>,
    #[serde(default)]
    position_value: Option<String>,
    #[serde(default)]
    position_im: Option<String>,
    #[serde(default)]
    position_mm: Option<String>,
    #[serde(default)]
    liq_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    unrealised_pnl: Option<String>,
    #[serde(default)]
    cum_realised_pnl: Option<String>,
    #[serde(default)]
    take_profit: Option<String>,
    #[serde(default)]
    stop_loss: Option<String>,
    #[serde(default)]
    trade_mode: Option<String>,
    updated_time: String,
}

/// 지갑 데이터
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitWalletData {
    #[serde(default)]
    account_type: Option<String>,
    #[serde(default)]
    coin: Vec<BybitCoinData>,
}

/// 코인 잔고 데이터
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitCoinData {
    coin: String,
    #[serde(default)]
    wallet_balance: Option<String>,
    #[serde(default)]
    available_to_withdraw: Option<String>,
    #[serde(default)]
    available_to_borrow: Option<String>,
    #[serde(default)]
    total_order_im: Option<String>,
    #[serde(default)]
    total_position_im: Option<String>,
    #[serde(default)]
    unrealised_pnl: Option<String>,
    #[serde(default)]
    cum_realised_pnl: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BybitWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(BybitWs::format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BybitWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(BybitWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BybitWs::format_interval(Timeframe::Minute1), "1");
        assert_eq!(BybitWs::format_interval(Timeframe::Hour1), "60");
        assert_eq!(BybitWs::format_interval(Timeframe::Day1), "D");
    }

    #[test]
    fn test_with_credentials() {
        let ws = BybitWs::with_credentials("my_api_key", "my_api_secret");
        assert_eq!(ws.get_api_key(), Some("my_api_key"));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = BybitWs::new();
        assert_eq!(ws.get_api_key(), None);
        ws.set_credentials("new_key", "new_secret");
        assert_eq!(ws.get_api_key(), Some("new_key"));
    }

    #[test]
    fn test_generate_auth_signature() {
        let expires = 1710125266669i64;
        let signature = BybitWs::generate_auth_signature("test_secret", expires);
        // Signature should be a 64-char hex string
        assert_eq!(signature.len(), 64);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_parse_order() {
        let data = BybitOrderData {
            symbol: "BTCUSDT".to_string(),
            order_id: "123456".to_string(),
            order_link_id: Some("client123".to_string()),
            side: "Buy".to_string(),
            order_type: "Limit".to_string(),
            price: "50000.0".to_string(),
            qty: "0.1".to_string(),
            time_in_force: Some("GTC".to_string()),
            order_status: "New".to_string(),
            cum_exec_qty: Some("0.0".to_string()),
            cum_exec_value: Some("0.0".to_string()),
            cum_exec_fee: Some("0.0".to_string()),
            leaves_qty: Some("0.1".to_string()),
            avg_price: None,
            reduce_only: Some(false),
            trigger_price: None,
            take_profit: None,
            stop_loss: None,
            updated_time: "1710125266669".to_string(),
        };

        let event = BybitWs::parse_order(&data);
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.id, "123456");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.price, Some(Decimal::from_str("50000.0").unwrap()));
        assert_eq!(event.order.time_in_force, Some(TimeInForce::GTC));
    }

    #[test]
    fn test_parse_execution() {
        let data = BybitExecutionData {
            symbol: "ETHUSDT".to_string(),
            exec_id: "exec123".to_string(),
            order_id: "order456".to_string(),
            order_link_id: None,
            side: "Sell".to_string(),
            exec_price: "3000.0".to_string(),
            exec_qty: "0.5".to_string(),
            exec_time: "1710125266669".to_string(),
            exec_fee: Some("0.15".to_string()),
            fee_currency: Some("USDT".to_string()),
            is_maker: Some(true),
        };

        let event = BybitWs::parse_execution(&data);
        assert_eq!(event.symbol, "ETH/USDT");
        assert_eq!(event.trades.len(), 1);
        assert_eq!(event.trades[0].id, "exec123");
        assert_eq!(event.trades[0].order, Some("order456".to_string()));
        assert_eq!(event.trades[0].side, Some("sell".to_string()));
        assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Maker));
        assert_eq!(event.trades[0].price, Decimal::from_str("3000.0").unwrap());
        assert_eq!(event.trades[0].amount, Decimal::from_str("0.5").unwrap());
    }

    #[test]
    fn test_parse_position() {
        let data = BybitPositionData {
            symbol: "BTCUSDT".to_string(),
            side: "Buy".to_string(),
            size: Some("0.1".to_string()),
            entry_price: Some("50000.0".to_string()),
            leverage: Some("10".to_string()),
            position_value: Some("5000.0".to_string()),
            position_im: Some("500.0".to_string()),
            position_mm: Some("50.0".to_string()),
            liq_price: Some("45000.0".to_string()),
            mark_price: Some("51000.0".to_string()),
            unrealised_pnl: Some("100.0".to_string()),
            cum_realised_pnl: Some("50.0".to_string()),
            take_profit: Some("55000.0".to_string()),
            stop_loss: Some("48000.0".to_string()),
            trade_mode: Some("0".to_string()),
            updated_time: "1710125266669".to_string(),
        };

        let event = BybitWs::parse_position(&data);
        assert_eq!(event.positions.len(), 1);
        let pos = &event.positions[0];
        assert_eq!(pos.symbol, "BTC/USDT");
        assert_eq!(pos.side, Some(PositionSide::Long));
        assert_eq!(pos.contracts, Some(Decimal::from_str("0.1").unwrap()));
        assert_eq!(pos.entry_price, Some(Decimal::from_str("50000.0").unwrap()));
        assert_eq!(pos.leverage, Some(Decimal::from_str("10").unwrap()));
        assert_eq!(pos.unrealized_pnl, Some(Decimal::from_str("100.0").unwrap()));
        assert_eq!(pos.margin_mode, Some(MarginMode::Cross));
    }

    #[test]
    fn test_parse_wallet() {
        let data = BybitWalletData {
            account_type: Some("UNIFIED".to_string()),
            coin: vec![
                BybitCoinData {
                    coin: "USDT".to_string(),
                    wallet_balance: Some("10000.0".to_string()),
                    available_to_withdraw: Some("9000.0".to_string()),
                    available_to_borrow: None,
                    total_order_im: Some("500.0".to_string()),
                    total_position_im: Some("500.0".to_string()),
                    unrealised_pnl: Some("100.0".to_string()),
                    cum_realised_pnl: Some("50.0".to_string()),
                },
                BybitCoinData {
                    coin: "BTC".to_string(),
                    wallet_balance: Some("1.5".to_string()),
                    available_to_withdraw: Some("1.0".to_string()),
                    available_to_borrow: None,
                    total_order_im: None,
                    total_position_im: None,
                    unrealised_pnl: None,
                    cum_realised_pnl: None,
                },
            ],
        };

        let event = BybitWs::parse_wallet(&data);
        assert!(event.balances.currencies.contains_key("USDT"));
        let usdt = event.balances.currencies.get("USDT").unwrap();
        assert_eq!(usdt.total, Some(Decimal::from_str("10000.0").unwrap()));
        assert_eq!(usdt.free, Some(Decimal::from_str("9000.0").unwrap()));
        assert!(event.balances.currencies.contains_key("BTC"));
        let btc = event.balances.currencies.get("BTC").unwrap();
        assert_eq!(btc.total, Some(Decimal::from_str("1.5").unwrap()));
    }

    #[test]
    fn test_process_order_message() {
        let json = r#"{
            "topic": "order",
            "data": [{
                "symbol": "BTCUSDT",
                "orderId": "12345",
                "side": "Buy",
                "orderType": "Limit",
                "price": "50000",
                "qty": "0.1",
                "orderStatus": "New",
                "updatedTime": "1710125266669"
            }]
        }"#;

        let result = BybitWs::process_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.side, OrderSide::Buy);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_execution_message() {
        let json = r#"{
            "topic": "execution",
            "data": [{
                "symbol": "ETHUSDT",
                "execId": "exec123",
                "orderId": "order456",
                "side": "Sell",
                "execPrice": "3000.0",
                "execQty": "0.5",
                "execTime": "1710125266669",
                "isMaker": false
            }]
        }"#;

        let result = BybitWs::process_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "ETH/USDT");
            assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Taker));
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_process_position_message() {
        let json = r#"{
            "topic": "position",
            "data": [{
                "symbol": "BTCUSDT",
                "side": "Sell",
                "size": "0.5",
                "entryPrice": "52000.0",
                "leverage": "5",
                "updatedTime": "1710125266669"
            }]
        }"#;

        let result = BybitWs::process_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert_eq!(event.positions[0].symbol, "BTC/USDT");
            assert_eq!(event.positions[0].side, Some(PositionSide::Short));
        } else {
            panic!("Expected Position message");
        }
    }

    #[test]
    fn test_process_auth_success() {
        let json = r#"{
            "op": "auth",
            "retCode": 0,
            "retMsg": "OK"
        }"#;

        let result = BybitWs::process_message(json);
        assert!(result.is_some());
        assert!(matches!(result, Some(WsMessage::Authenticated)));
    }

    #[test]
    fn test_process_auth_failure() {
        let json = r#"{
            "op": "auth",
            "retCode": 10003,
            "retMsg": "Invalid API key"
        }"#;

        let result = BybitWs::process_message(json);
        assert!(result.is_some());
        if let Some(WsMessage::Error(msg)) = result {
            assert!(msg.contains("Authentication failed"));
        } else {
            panic!("Expected Error message");
        }
    }
}
