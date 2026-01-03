//! Gate.io WebSocket Implementation
//!
//! Gate.io 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    Balance, Balances, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Position, PositionSide, TakerOrMaker, Ticker, Timeframe, TimeInForce, Trade, OHLCV,
    WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent, WsOhlcvEvent, WsOrderBookEvent,
    WsOrderEvent, WsPositionEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha512 = Hmac<Sha512>;

const WS_PUBLIC_URL: &str = "wss://api.gateio.ws/ws/v4/";
const WS_PRIVATE_URL: &str = "wss://api.gateio.ws/ws/v4/";

/// Gate.io WebSocket 클라이언트
pub struct GateWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl GateWs {
    /// 새 Gate.io WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
        }
    }

    /// API 자격 증명으로 생성
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key),
            api_secret: Some(api_secret),
        }
    }

    /// API 자격 증명 설정
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// API 키 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Generate signature for authentication
    /// Gate.io uses HMAC-SHA512
    /// message = "channel={channel}&event=subscribe&time={timestamp}"
    fn generate_auth_signature(api_secret: &str, channel: &str, event: &str, timestamp: i64) -> String {
        let message = format!("channel={}&event={}&time={}", channel, event, timestamp);
        let mut mac = HmacSha512::new_from_slice(api_secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// 심볼을 Gate 형식으로 변환 (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Gate 심볼을 통합 심볼로 변환 (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(gate_symbol: &str) -> String {
        gate_symbol.replace("_", "/")
    }

    /// Timeframe을 Gate 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Second1 => "10s",
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "7d",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &GateTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.time.map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.highest_bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.lowest_ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change_percentage.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.change_percentage.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.base_volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &GateOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let timestamp = data.t
            .unwrap_or_else(|| Utc::now().timestamp_millis());

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
    fn parse_trade(data: &GateTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.create_time.map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.side.to_lowercase()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol, trades }
    }

    /// OHLCV 메시지 파싱
    fn parse_candle(data: &GateCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let timestamp = data.t.parse::<i64>().ok()? * 1000;
        let ohlcv = OHLCV::new(
            timestamp,
            data.o.parse().ok()?,
            data.h.parse().ok()?,
            data.l.parse().ok()?,
            data.c.parse().ok()?,
            data.v.parse().ok()?,
        );

        let symbol = Self::to_unified_symbol(&data.n);

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 주문 메시지 파싱
    fn parse_order(data: &GateOrderData) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.update_time_ms.unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.side.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref().unwrap_or("limit").to_lowercase().as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = match data.status.to_lowercase().as_str() {
            "open" => OrderStatus::Open,
            "closed" => OrderStatus::Closed,
            "cancelled" | "canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let time_in_force = match data.time_in_force.as_deref().unwrap_or("gtc").to_lowercase().as_str() {
            "gtc" => TimeInForce::GTC,
            "ioc" => TimeInForce::IOC,
            "poc" => TimeInForce::PO,
            "fok" => TimeInForce::FOK,
            _ => TimeInForce::GTC,
        };

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();
        let filled: Decimal = data.filled_total.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let order = Order {
            id: data.id.clone(),
            client_order_id: data.text.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            status,
            symbol: symbol.clone(),
            order_type,
            time_in_force: Some(time_in_force),
            side,
            price: Some(price),
            average: data.avg_deal_price.as_ref().and_then(|v| v.parse().ok()),
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: Some(filled * price),
            trades: Vec::new(),
            fee: data.fee.as_ref().map(|f| Fee {
                currency: data.fee_currency.clone(),
                cost: f.parse().ok(),
                rate: None,
            }),
            fees: Vec::new(),
            reduce_only: None,
            post_only: Some(data.time_in_force.as_deref() == Some("poc")),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 체결 메시지 파싱 (Private)
    fn parse_fill(data: &GateFillData) -> WsMyTradeEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.create_time_ms.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        let side = match data.side.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let taker_or_maker = match data.role.to_lowercase().as_str() {
            "taker" => TakerOrMaker::Taker,
            "maker" => TakerOrMaker::Maker,
            _ => TakerOrMaker::Taker,
        };

        let trades = vec![Trade {
            id: data.id.to_string(),
            order: Some(data.order_id.clone()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(if side == OrderSide::Buy { "buy".to_string() } else { "sell".to_string() }),
            taker_or_maker: Some(taker_or_maker),
            price,
            amount,
            cost: Some(price * amount),
            fee: data.fee.as_ref().map(|f| Fee {
                currency: data.fee_currency.clone(),
                cost: f.parse().ok(),
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsMyTradeEvent { symbol, trades }
    }

    /// 포지션 메시지 파싱
    fn parse_position(data: &GatePositionData) -> WsPositionEvent {
        let symbol = Self::to_unified_symbol(&data.contract);
        let timestamp = data.update_time.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = if data.size > 0 {
            PositionSide::Long
        } else if data.size < 0 {
            PositionSide::Short
        } else {
            PositionSide::Unknown
        };

        let margin_mode = match data.mode.as_deref().unwrap_or("").to_lowercase().as_str() {
            "single" | "isolated" => MarginMode::Isolated,
            "dual" | "cross" => MarginMode::Cross,
            _ => MarginMode::Unknown,
        };

        let size = Decimal::from(data.size.abs());
        let entry_price: Decimal = data.entry_price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
        let mark_price: Decimal = data.mark_price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let position = Position {
            id: None,
            symbol: symbol.clone(),
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            contracts: Some(size),
            contract_size: None,
            side: Some(side),
            notional: Some(size * mark_price),
            leverage: data.leverage.and_then(|l| Decimal::try_from(l).ok()),
            unrealized_pnl: data.unrealised_pnl.as_ref().and_then(|v| v.parse().ok()),
            realized_pnl: None,
            collateral: data.margin.as_ref().and_then(|v| v.parse().ok()),
            entry_price: Some(entry_price),
            mark_price: Some(mark_price),
            liquidation_price: data.liq_price.as_ref().and_then(|v| v.parse().ok()),
            margin_mode: Some(margin_mode),
            hedged: None,
            maintenance_margin: data.maintenance_rate.as_ref().and_then(|v| v.parse().ok()),
            maintenance_margin_percentage: data.maintenance_rate.as_ref().and_then(|v| v.parse().ok()),
            initial_margin: data.margin.as_ref().and_then(|v| v.parse().ok()),
            initial_margin_percentage: None,
            margin_ratio: None,
            last_update_timestamp: Some(timestamp),
            last_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            percentage: None,
        };

        WsPositionEvent {
            positions: vec![position],
        }
    }

    /// 잔고 메시지 파싱
    fn parse_account(data: &[GateBalanceData]) -> WsBalanceEvent {
        let timestamp = Utc::now().timestamp_millis();
        let mut currencies_map: HashMap<String, Balance> = HashMap::new();

        for item in data {
            let currency = item.currency.clone();
            let free: Decimal = item.available.parse().unwrap_or_default();
            let used: Decimal = item.freeze.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
            let total = free + used;

            currencies_map.insert(currency, Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            });
        }

        let balances = Balances {
            info: serde_json::to_value(data).unwrap_or_default(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies: currencies_map,
        };

        WsBalanceEvent { balances }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<GateWsResponse>(msg) {
            // Event handling
            if let Some(event) = &response.event {
                if event == "subscribe" {
                    // Check for auth success (spot.orders with auth)
                    if let Some(channel) = &response.channel {
                        if channel == "spot.orders" || channel == "spot.usertrades" ||
                           channel == "spot.balances" || channel == "futures.orders" ||
                           channel == "futures.usertrades" || channel == "futures.positions" ||
                           channel == "futures.balances" {
                            // Check if error
                            if response.error.is_some() {
                                return Some(WsMessage::Error(
                                    response.error.as_ref()
                                        .and_then(|e| e.message.clone())
                                        .unwrap_or_else(|| "Auth failed".to_string())
                                ));
                            }
                            return Some(WsMessage::Authenticated);
                        }
                    }
                    return Some(WsMessage::Subscribed {
                        channel: response.channel.clone().unwrap_or_default(),
                        symbol: None,
                    });
                }
            }

            // Data handling
            if let (Some(channel), Some(result)) = (&response.channel, &response.result) {
                // Ticker
                if channel == "spot.tickers" {
                    if let Ok(ticker_data) = serde_json::from_value::<GateTickerData>(result.clone()) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }

                // OrderBook
                if channel == "spot.order_book_update" {
                    if let Ok(ob_data) = serde_json::from_value::<GateOrderBookData>(result.clone()) {
                        let symbol = ob_data.s.as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        let is_snapshot = response.event.as_deref() == Some("all");
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                    }
                }

                // Trades
                if channel == "spot.trades" {
                    if let Ok(trade_data) = serde_json::from_value::<GateTradeData>(result.clone()) {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                    }
                }

                // Candles
                if channel == "spot.candlesticks" {
                    if let Ok(candle_data) = serde_json::from_value::<GateCandleData>(result.clone()) {
                        // Extract timeframe from candle name (e.g., "1m_BTC_USDT")
                        let parts: Vec<&str> = candle_data.n.split('_').collect();
                        let timeframe = if !parts.is_empty() {
                            match parts[0] {
                                "10s" => Timeframe::Second1,
                                "1m" => Timeframe::Minute1,
                                "3m" => Timeframe::Minute3,
                                "5m" => Timeframe::Minute5,
                                "15m" => Timeframe::Minute15,
                                "30m" => Timeframe::Minute30,
                                "1h" => Timeframe::Hour1,
                                "2h" => Timeframe::Hour2,
                                "4h" => Timeframe::Hour4,
                                "6h" => Timeframe::Hour6,
                                "8h" => Timeframe::Hour8,
                                "12h" => Timeframe::Hour12,
                                "1d" => Timeframe::Day1,
                                "7d" => Timeframe::Week1,
                                _ => Timeframe::Minute1,
                            }
                        } else {
                            Timeframe::Minute1
                        };

                        if let Some(event) = Self::parse_candle(&candle_data, timeframe) {
                            return Some(WsMessage::Ohlcv(event));
                        }
                    }
                }

                // Private Channels - Orders
                if channel == "spot.orders" || channel == "futures.orders" {
                    if let Ok(order_data) = serde_json::from_value::<Vec<GateOrderData>>(result.clone()) {
                        if let Some(order) = order_data.first() {
                            return Some(WsMessage::Order(Self::parse_order(order)));
                        }
                    }
                }

                // Private Channels - User Trades
                if channel == "spot.usertrades" || channel == "futures.usertrades" {
                    if let Ok(fill_data) = serde_json::from_value::<Vec<GateFillData>>(result.clone()) {
                        if let Some(fill) = fill_data.first() {
                            return Some(WsMessage::MyTrade(Self::parse_fill(fill)));
                        }
                    }
                }

                // Private Channels - Positions (Futures only)
                if channel == "futures.positions" {
                    if let Ok(position_data) = serde_json::from_value::<Vec<GatePositionData>>(result.clone()) {
                        if let Some(position) = position_data.first() {
                            return Some(WsMessage::Position(Self::parse_position(position)));
                        }
                    }
                }

                // Private Channels - Balance
                if channel == "spot.balances" || channel == "futures.balances" {
                    if let Ok(balance_data) = serde_json::from_value::<Vec<GateBalanceData>>(result.clone()) {
                        return Some(WsMessage::Balance(Self::parse_account(&balance_data)));
                    }
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, channel: &str, payload: Vec<String>, channel_name: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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
        let timestamp = Utc::now().timestamp();
        let subscribe_msg = serde_json::json!({
            "time": timestamp,
            "channel": channel,
            "event": "subscribe",
            "payload": payload
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel_name, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
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

    /// Private 스트림 구독 (인증 포함)
    async fn subscribe_private_stream(
        &mut self,
        channel: &str,
        payload: Vec<String>,
        channel_name: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self.api_key.clone().ok_or_else(|| {
            crate::errors::CcxtError::AuthenticationError {
                message: "API key not set".into(),
            }
        })?;
        let api_secret = self.api_secret.clone().ok_or_else(|| {
            crate::errors::CcxtError::AuthenticationError {
                message: "API secret not set".into(),
            }
        })?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PRIVATE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Generate auth signature
        let timestamp = Utc::now().timestamp();
        let sign = Self::generate_auth_signature(&api_secret, channel, "subscribe", timestamp);

        // Subscribe with auth
        let subscribe_msg = serde_json::json!({
            "time": timestamp,
            "channel": channel,
            "event": "subscribe",
            "payload": payload,
            "auth": {
                "method": "api_key",
                "KEY": api_key,
                "SIGN": sign
            }
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Save subscription
        {
            let key = format!("private:{}:{}", channel_name, "");
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Event processing task
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

impl Default for GateWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for GateWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        client.subscribe_stream("spot.tickers", vec![market_id], "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_ids: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client.subscribe_stream("spot.tickers", market_ids, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        // Gate uses "100ms" for real-time updates
        client.subscribe_stream("spot.order_book_update", vec![market_id, "100ms".to_string()], "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        client.subscribe_stream("spot.trades", vec![market_id], "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscription = format!("{interval}_{market_id}");
        client.subscribe_stream("spot.candlesticks", vec![subscription], "ohlcv", Some(symbol)).await
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

    // === Private Streams ===

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap_or_default(),
            self.api_secret.clone().unwrap_or_default(),
        );
        let payload = if let Some(sym) = symbol {
            vec![Self::format_symbol(sym)]
        } else {
            vec!["!all".to_string()]
        };
        client.subscribe_private_stream("spot.orders", payload, "orders").await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap_or_default(),
            self.api_secret.clone().unwrap_or_default(),
        );
        let payload = if let Some(sym) = symbol {
            vec![Self::format_symbol(sym)]
        } else {
            vec!["!all".to_string()]
        };
        client.subscribe_private_stream("spot.usertrades", payload, "usertrades").await
    }

    async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap_or_default(),
            self.api_secret.clone().unwrap_or_default(),
        );
        let payload = if let Some(syms) = symbols {
            syms.iter().map(|s| Self::format_symbol(s)).collect()
        } else {
            vec!["!all".to_string()]
        };
        client.subscribe_private_stream("futures.positions", payload, "positions").await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.clone().unwrap_or_default(),
            self.api_secret.clone().unwrap_or_default(),
        );
        client.subscribe_private_stream("spot.balances", vec![], "balances").await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(crate::errors::CcxtError::AuthenticationError {
                message: "API credentials not set".into(),
            });
        }
        // Gate.io includes auth in subscription message, no separate auth needed
        Ok(())
    }
}

// === Gate.io WebSocket Types ===

#[derive(Debug, Deserialize)]
struct GateWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<GateErrorData>,
}

#[derive(Debug, Deserialize)]
struct GateErrorData {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateTickerData {
    currency_pair: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    lowest_ask: Option<String>,
    #[serde(default)]
    highest_bid: Option<String>,
    #[serde(default)]
    change_percentage: Option<String>,
    #[serde(default)]
    base_volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(default)]
    low_24h: Option<String>,
    #[serde(default)]
    time: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateOrderBookData {
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    u: Option<i64>,
    #[serde(default)]
    s: Option<String>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateTradeData {
    id: i64,
    currency_pair: String,
    price: String,
    amount: String,
    side: String,
    #[serde(default)]
    create_time: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateCandleData {
    t: String,  // timestamp
    o: String,  // open
    h: String,  // high
    l: String,  // low
    c: String,  // close
    v: String,  // volume
    n: String,  // name (interval_symbol)
}

// === Private Data Types ===

#[derive(Debug, Deserialize, Serialize)]
struct GateOrderData {
    id: String,
    currency_pair: String,
    #[serde(default)]
    text: Option<String>,
    side: String,
    #[serde(default)]
    order_type: Option<String>,
    status: String,
    price: String,
    amount: String,
    #[serde(default)]
    filled_total: Option<String>,
    #[serde(default)]
    avg_deal_price: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    update_time_ms: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateFillData {
    id: i64,
    order_id: String,
    currency_pair: String,
    side: String,
    role: String,
    price: String,
    amount: String,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    create_time_ms: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GatePositionData {
    contract: String,
    size: i64,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    leverage: Option<f64>,
    #[serde(default)]
    entry_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    unrealised_pnl: Option<String>,
    #[serde(default)]
    margin: Option<String>,
    #[serde(default)]
    maintenance_rate: Option<String>,
    #[serde(default)]
    liq_price: Option<String>,
    #[serde(default)]
    update_time: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateBalanceData {
    currency: String,
    available: String,
    #[serde(default)]
    freeze: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(GateWs::format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(GateWs::format_symbol("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(GateWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(GateWs::to_unified_symbol("ETH_BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(GateWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(GateWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(GateWs::format_interval(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_with_credentials() {
        let client = GateWs::with_credentials(
            "test_key".to_string(),
            "test_secret".to_string(),
        );
        assert_eq!(client.get_api_key(), Some("test_key"));
    }

    #[test]
    fn test_set_credentials() {
        let mut client = GateWs::new();
        assert_eq!(client.get_api_key(), None);
        client.set_credentials("key".to_string(), "secret".to_string());
        assert_eq!(client.get_api_key(), Some("key"));
    }

    #[test]
    fn test_generate_auth_signature() {
        let signature = GateWs::generate_auth_signature(
            "test_secret",
            "spot.orders",
            "subscribe",
            1234567890,
        );
        // Gate uses HMAC-SHA512, signature should be hex-encoded
        assert!(!signature.is_empty());
        assert_eq!(signature.len(), 128); // SHA512 produces 64 bytes = 128 hex chars
    }

    #[test]
    fn test_parse_order() {
        let order_data = GateOrderData {
            id: "123456".to_string(),
            currency_pair: "BTC_USDT".to_string(),
            text: Some("client_order_1".to_string()),
            side: "buy".to_string(),
            order_type: Some("limit".to_string()),
            status: "open".to_string(),
            price: "50000.0".to_string(),
            amount: "1.0".to_string(),
            filled_total: Some("0.5".to_string()),
            avg_deal_price: Some("50000.0".to_string()),
            fee: Some("0.001".to_string()),
            fee_currency: Some("USDT".to_string()),
            time_in_force: Some("gtc".to_string()),
            update_time_ms: Some(1700000000000),
        };
        let event = GateWs::parse_order(&order_data);
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.id, "123456");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
    }

    #[test]
    fn test_parse_fill() {
        let fill_data = GateFillData {
            id: 789,
            order_id: "123456".to_string(),
            currency_pair: "ETH_USDT".to_string(),
            side: "sell".to_string(),
            role: "maker".to_string(),
            price: "2000.0".to_string(),
            amount: "0.5".to_string(),
            fee: Some("0.001".to_string()),
            fee_currency: Some("USDT".to_string()),
            create_time_ms: Some(1700000000000),
        };
        let event = GateWs::parse_fill(&fill_data);
        assert_eq!(event.symbol, "ETH/USDT");
        assert_eq!(event.trades.len(), 1);
        assert_eq!(event.trades[0].id, "789");
        assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Maker));
    }

    #[test]
    fn test_parse_position() {
        let position_data = GatePositionData {
            contract: "BTC_USDT".to_string(),
            size: 100,
            mode: Some("single".to_string()),
            leverage: Some(10.0),
            entry_price: Some("50000.0".to_string()),
            mark_price: Some("51000.0".to_string()),
            unrealised_pnl: Some("1000.0".to_string()),
            margin: Some("5000.0".to_string()),
            maintenance_rate: Some("0.004".to_string()),
            liq_price: Some("45000.0".to_string()),
            update_time: Some(1700000000),
        };
        let event = GateWs::parse_position(&position_data);
        assert_eq!(event.positions.len(), 1);
        assert_eq!(event.positions[0].symbol, "BTC/USDT");
        assert_eq!(event.positions[0].side, Some(PositionSide::Long));
        assert_eq!(event.positions[0].margin_mode, Some(MarginMode::Isolated));
    }

    #[test]
    fn test_parse_account() {
        let balance_data = vec![
            GateBalanceData {
                currency: "BTC".to_string(),
                available: "1.5".to_string(),
                freeze: Some("0.5".to_string()),
            },
            GateBalanceData {
                currency: "USDT".to_string(),
                available: "10000.0".to_string(),
                freeze: Some("1000.0".to_string()),
            },
        ];
        let event = GateWs::parse_account(&balance_data);
        assert_eq!(event.balances.currencies.len(), 2);
        let btc = event.balances.currencies.get("BTC").unwrap();
        assert_eq!(btc.free, Some(Decimal::new(15, 1)));
        assert_eq!(btc.used, Some(Decimal::new(5, 1)));
    }

    #[test]
    fn test_process_order_message() {
        let msg = r#"{
            "channel": "spot.orders",
            "event": "update",
            "result": [{
                "id": "123456",
                "currency_pair": "BTC_USDT",
                "side": "buy",
                "status": "open",
                "price": "50000.0",
                "amount": "1.0"
            }]
        }"#;
        let result = GateWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.symbol, "BTC/USDT");
            assert_eq!(event.order.id, "123456");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_fill_message() {
        let msg = r#"{
            "channel": "spot.usertrades",
            "event": "update",
            "result": [{
                "id": 789,
                "order_id": "123456",
                "currency_pair": "ETH_USDT",
                "side": "buy",
                "role": "taker",
                "price": "2000.0",
                "amount": "0.5"
            }]
        }"#;
        let result = GateWs::process_message(msg);
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
        let msg = r#"{
            "channel": "futures.positions",
            "event": "update",
            "result": [{
                "contract": "BTC_USDT",
                "size": -50,
                "mode": "dual",
                "leverage": 5.0,
                "entry_price": "50000.0",
                "mark_price": "49000.0"
            }]
        }"#;
        let result = GateWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert_eq!(event.positions[0].symbol, "BTC/USDT");
            assert_eq!(event.positions[0].side, Some(PositionSide::Short));
        } else {
            panic!("Expected Position message");
        }
    }

    #[test]
    fn test_process_balance_message() {
        let msg = r#"{
            "channel": "spot.balances",
            "event": "update",
            "result": [{
                "currency": "USDT",
                "available": "5000.0",
                "freeze": "500.0"
            }]
        }"#;
        let result = GateWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            let usdt = event.balances.currencies.get("USDT").unwrap();
            assert_eq!(usdt.free, Some(Decimal::new(5000, 0)));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_auth_success() {
        let msg = r#"{
            "channel": "spot.orders",
            "event": "subscribe"
        }"#;
        let result = GateWs::process_message(msg);
        assert!(result.is_some());
        assert!(matches!(result, Some(WsMessage::Authenticated)));
    }

    #[test]
    fn test_process_auth_failure() {
        let msg = r#"{
            "channel": "spot.orders",
            "event": "subscribe",
            "error": {
                "code": 2,
                "message": "Invalid signature"
            }
        }"#;
        let result = GateWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Error(err)) = result {
            assert_eq!(err, "Invalid signature");
        } else {
            panic!("Expected Error message");
        }
    }
}
