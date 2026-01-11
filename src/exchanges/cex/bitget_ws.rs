//! Bitget WebSocket Implementation
//!
//! Bitget 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
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

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://ws.bitget.com/spot/v1/stream";
const WS_PRIVATE_URL: &str = "wss://ws.bitget.com/spot/v1/stream";

/// Bitget WebSocket 클라이언트
pub struct BitgetWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
}

impl BitgetWs {
    /// 새 Bitget WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: None,
            api_secret: None,
            passphrase: None,
        }
    }

    /// API 자격 증명으로 생성
    pub fn with_credentials(api_key: &str, api_secret: &str, passphrase: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: Some(api_key.to_string()),
            api_secret: Some(api_secret.to_string()),
            passphrase: Some(passphrase.to_string()),
        }
    }

    /// API 자격 증명 설정
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str, passphrase: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
        self.passphrase = Some(passphrase.to_string());
    }

    /// API 키 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// WebSocket 인증 서명 생성
    /// sign = Base64(HMAC_SHA256(timestamp + "GET" + "/user/verify", secretKey))
    fn generate_auth_signature(api_secret: &str, timestamp: &str) -> String {
        let sign_str = format!("{timestamp}GET/user/verify");
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_str.as_bytes());
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, mac.finalize().into_bytes())
    }

    /// 심볼을 Bitget 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Bitget 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(bitget_symbol: &str) -> String {
        // Common quote currencies
        for quote in &["USDT", "USDC", "BTC", "ETH"] {
            if let Some(base) = bitget_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }
        bitget_symbol.to_string()
    }

    /// Timeframe을 Bitget 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1H",
            Timeframe::Hour4 => "4H",
            Timeframe::Hour6 => "6H",
            Timeframe::Hour12 => "12H",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BitgetTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid_pr.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_sz.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_pr.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_sz.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last_pr.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_pr.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.change_utc.as_ref().and_then(|v| {
                v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))
            }),
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
    fn parse_order_book(data: &BitgetOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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

        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
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
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BitgetTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
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

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// OHLCV 메시지 파싱
    fn parse_candle(data: &[String], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        if data.len() < 6 {
            return None;
        }

        let timestamp = data[0].parse::<i64>().ok()?;
        let ohlcv = OHLCV::new(
            timestamp,
            data[1].parse().ok()?,
            data[2].parse().ok()?,
            data[3].parse().ok()?,
            data[4].parse().ok()?,
            data[5].parse().ok()?,
        );

        let unified_symbol = Self::to_unified_symbol(symbol);

        Some(WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        })
    }

    // === Private Channel Parsing ===

    /// 주문 메시지 파싱
    fn parse_order(data: &BitgetOrderData) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.u_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.side.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.ord_type.to_lowercase().as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = match data.status.to_lowercase().as_str() {
            "new" | "init" | "pending" => OrderStatus::Open,
            "partial-fill" | "partially_filled" => OrderStatus::Open,
            "full-fill" | "filled" => OrderStatus::Closed,
            "cancelled" | "canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let time_in_force = match data.force.as_deref().unwrap_or("").to_lowercase().as_str() {
            "gtc" => TimeInForce::GTC,
            "ioc" => TimeInForce::IOC,
            "fok" => TimeInForce::FOK,
            "po" | "post_only" => TimeInForce::PO,
            _ => TimeInForce::GTC,
        };

        let price: Decimal = data.px.as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data.sz.as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data.fill_sz.as_ref()
            .and_then(|f| f.parse().ok())
            .unwrap_or_default();
        let average: Option<Decimal> = data.avg_px.as_ref().and_then(|a| a.parse().ok());
        let remaining = amount - filled;
        let cost = average.map(|avg| avg * filled);

        let fee = data.fee.as_ref().and_then(|f| f.parse::<Decimal>().ok()).map(|cost| Fee {
            cost: Some(cost.abs()),
            currency: data.fee_ccy.clone(),
            rate: None,
        });

        let order = Order {
            id: data.ord_id.clone(),
            client_order_id: data.cl_ord_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            status,
            symbol,
            order_type,
            time_in_force: Some(time_in_force),
            side,
            price: Some(price),
            average,
            amount,
            filled,
            remaining: Some(remaining),
            trigger_price: None,
            stop_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            cost,
            trades: Vec::new(),
            reduce_only: data.reduce_only.as_deref().map(|r| r == "true"),
            post_only: Some(matches!(time_in_force, TimeInForce::PO)),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 체결 메시지 파싱 (private fill)
    fn parse_fill(data: &BitgetFillData) -> WsMyTradeEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.fill_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let price: Decimal = data.fill_px.as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data.fill_sz.as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let fee = data.fee.as_ref().and_then(|f| f.parse::<Decimal>().ok()).map(|cost| Fee {
            cost: Some(cost.abs()),
            currency: data.fee_ccy.clone(),
            rate: None,
        });
        let fees = fee.clone().map(|f| vec![f]).unwrap_or_default();

        let taker_or_maker = data.exec_type.as_ref().map(|e| {
            if e.to_lowercase() == "taker" || e == "T" { TakerOrMaker::Taker } else { TakerOrMaker::Maker }
        });

        let trade = Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: data.ord_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.side.to_lowercase()),
            taker_or_maker,
            price,
            amount,
            cost: Some(price * amount),
            fee,
            fees,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsMyTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// 포지션 메시지 파싱
    fn parse_position(data: &BitgetPositionData) -> WsPositionEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.u_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let pos_side = match data.hold_side.as_deref().unwrap_or("").to_lowercase().as_str() {
            "long" => PositionSide::Long,
            "short" => PositionSide::Short,
            _ => PositionSide::Unknown,
        };

        let margin_mode = match data.margin_mode.as_deref().unwrap_or("").to_lowercase().as_str() {
            "isolated" | "fixed" => MarginMode::Isolated,
            "crossed" | "cross" => MarginMode::Cross,
            _ => MarginMode::Unknown,
        };

        let contracts: Option<Decimal> = data.total.as_ref().and_then(|t| t.parse().ok());
        let entry_price: Option<Decimal> = data.avg_open_price.as_ref().and_then(|a| a.parse().ok());
        let mark_price: Option<Decimal> = data.mark_price.as_ref().and_then(|m| m.parse().ok());
        let unrealized_pnl: Option<Decimal> = data.unrealized_pl.as_ref().and_then(|u| u.parse().ok());
        let leverage: Option<Decimal> = data.leverage.as_ref().and_then(|l| l.parse().ok());
        let liquidation_price: Option<Decimal> = data.liq_px.as_ref().and_then(|l| l.parse().ok());
        let margin: Option<Decimal> = data.margin.as_ref().and_then(|m| m.parse().ok());

        let position = Position {
            id: data.pos_id.clone(),
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            contracts,
            contract_size: None,
            side: Some(pos_side),
            notional: None,
            leverage,
            unrealized_pnl,
            realized_pnl: data.achieved_profits.as_ref().and_then(|r| r.parse().ok()),
            collateral: margin,
            entry_price,
            mark_price,
            liquidation_price,
            margin_mode: Some(margin_mode),
            hedged: None,
            margin_ratio: data.margin_ratio.as_ref().and_then(|m| m.parse().ok()),
            initial_margin: None,
            initial_margin_percentage: None,
            maintenance_margin: None,
            maintenance_margin_percentage: None,
            last_update_timestamp: Some(timestamp),
            last_price: mark_price,
            stop_loss_price: None,
            take_profit_price: None,
            percentage: data.upl_rate.as_ref().and_then(|u| u.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsPositionEvent {
            positions: vec![position],
        }
    }

    /// 계정 잔고 메시지 파싱
    fn parse_account(data: &BitgetAccountData) -> WsBalanceEvent {
        let timestamp = Utc::now().timestamp_millis();

        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for detail in &data.details {
            let free: Decimal = detail.available.as_ref()
                .and_then(|a| a.parse().ok())
                .unwrap_or_default();
            let used: Decimal = detail.frozen.as_ref()
                .and_then(|f| f.parse().ok())
                .unwrap_or_default();
            let total = free + used;

            currencies.insert(detail.coin.clone(), Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            });
        }

        let balances = Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsBalanceEvent { balances }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<BitgetWsResponse>(msg) {
            // Login response handling
            if let Some(event) = &response.event {
                if event == "login" {
                    if response.code.as_ref().map(|c| c == "0").unwrap_or(false) {
                        return Some(WsMessage::Authenticated);
                    } else {
                        return Some(WsMessage::Error(
                            format!("Auth failed: {}", response.msg.as_deref().unwrap_or("Unknown error"))
                        ));
                    }
                }
            }

            // Event handling (subscribe confirmation)
            if response.event.as_deref() == Some("subscribe") {
                return Some(WsMessage::Subscribed {
                    channel: response.arg.as_ref()
                        .and_then(|a| a.channel.clone())
                        .unwrap_or_default(),
                    symbol: response.arg.as_ref().and_then(|a| a.inst_id.clone()),
                });
            }

            // Data handling
            if let (Some(arg), Some(data_arr)) = (&response.arg, &response.data) {
                let channel = arg.channel.as_deref().unwrap_or("");
                let symbol = arg.inst_id.as_deref().unwrap_or("");

                // === Public Channels ===

                // Ticker
                if channel == "ticker" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ticker_data) = serde_json::from_value::<BitgetTickerData>(first.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }
                }

                // OrderBook
                if channel == "books" || channel == "books5" || channel == "books15" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ob_data) = serde_json::from_value::<BitgetOrderBookData>(first.clone()) {
                            let unified_symbol = Self::to_unified_symbol(symbol);
                            let is_snapshot = response.action.as_deref() == Some("snapshot");
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &unified_symbol, is_snapshot)));
                        }
                    }
                }

                // Trade
                if channel == "trade" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(trade_data) = serde_json::from_value::<BitgetTradeData>(first.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data, symbol)));
                        }
                    }
                }

                // Candle
                if channel.starts_with("candle") {
                    if let Some(first) = data_arr.first() {
                        if let Ok(candle_arr) = serde_json::from_value::<Vec<String>>(first.clone()) {
                            let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                            let sym = subscribed_symbol.unwrap_or(symbol);
                            if let Some(event) = Self::parse_candle(&candle_arr, sym, timeframe) {
                                return Some(WsMessage::Ohlcv(event));
                            }
                        }
                    }
                }

                // === Private Channels ===

                // Orders
                if channel == "orders" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(order_data) = serde_json::from_value::<BitgetOrderData>(first.clone()) {
                            return Some(WsMessage::Order(Self::parse_order(&order_data)));
                        }
                    }
                }

                // Fills (MyTrades)
                if channel == "fill" || channel == "fills" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(fill_data) = serde_json::from_value::<BitgetFillData>(first.clone()) {
                            return Some(WsMessage::MyTrade(Self::parse_fill(&fill_data)));
                        }
                    }
                }

                // Positions
                if channel == "positions" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(position_data) = serde_json::from_value::<BitgetPositionData>(first.clone()) {
                            return Some(WsMessage::Position(Self::parse_position(&position_data)));
                        }
                    }
                }

                // Account
                if channel == "account" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(account_data) = serde_json::from_value::<BitgetAccountData>(first.clone()) {
                            return Some(WsMessage::Balance(Self::parse_account(&account_data)));
                        }
                    }
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        args: Vec<serde_json::Value>,
        channel: &str,
        symbol: Option<&str>,
        timeframe: Option<Timeframe>
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
            "op": "subscribe",
            "args": args
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let subscribed_symbol = symbol.map(|s| s.to_string());
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
                        if let Some(ws_msg) = Self::process_message(&msg, subscribed_symbol.as_deref(), timeframe) {
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

    /// 비공개 스트림 구독 (인증 필요)
    async fn subscribe_private_stream(
        &mut self,
        args: Vec<serde_json::Value>,
        channel: &str,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self.api_key.clone().ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
            message: "API key not set".into(),
        })?;
        let api_secret = self.api_secret.clone().ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
            message: "API secret not set".into(),
        })?;
        let passphrase = self.passphrase.clone().ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
            message: "Passphrase not set".into(),
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

        // Generate timestamp (seconds)
        let timestamp = (Utc::now().timestamp_millis() / 1000).to_string();
        let sign = Self::generate_auth_signature(&api_secret, &timestamp);

        // Send login message
        let login_msg = serde_json::json!({
            "op": "login",
            "args": [{
                "apiKey": api_key,
                "passphrase": passphrase,
                "timestamp": timestamp,
                "sign": sign
            }]
        });
        ws_client.send(&login_msg.to_string())?;

        // Send subscribe message
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": args
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Event processing task
        let tx = event_tx.clone();
        let subscribed_symbol = symbol.map(|s| s.to_string());
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
                        if let Some(ws_msg) = Self::process_message(&msg, subscribed_symbol.as_deref(), None) {
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

impl Default for BitgetWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitgetWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args = vec![serde_json::json!({
            "instType": "sp",
            "channel": "ticker",
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "ticker", Some(symbol), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args: Vec<serde_json::Value> = symbols
            .iter()
            .map(|s| serde_json::json!({
                "instType": "sp",
                "channel": "ticker",
                "instId": Self::format_symbol(s)
            }))
            .collect();
        client.subscribe_stream(args, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = match limit.unwrap_or(15) {
            1..=5 => "books5",
            6..=15 => "books15",
            _ => "books",
        };
        let args = vec![serde_json::json!({
            "instType": "sp",
            "channel": channel,
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "orderBook", Some(symbol), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args = vec![serde_json::json!({
            "instType": "sp",
            "channel": "trade",
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "trades", Some(symbol), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let args = vec![serde_json::json!({
            "instType": "sp",
            "channel": format!("candle{}", interval),
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "ohlcv", Some(symbol), Some(timeframe)).await
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

    // === Private Channels ===

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.as_deref().unwrap_or(""),
            self.api_secret.as_deref().unwrap_or(""),
            self.passphrase.as_deref().unwrap_or(""),
        );
        let args = if let Some(sym) = symbol {
            vec![serde_json::json!({
                "instType": "spbl",
                "channel": "orders",
                "instId": Self::format_symbol(sym)
            })]
        } else {
            vec![serde_json::json!({
                "instType": "spbl",
                "channel": "orders",
                "instId": "default"
            })]
        };
        client.subscribe_private_stream(args, "orders", symbol).await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.as_deref().unwrap_or(""),
            self.api_secret.as_deref().unwrap_or(""),
            self.passphrase.as_deref().unwrap_or(""),
        );
        let args = if let Some(sym) = symbol {
            vec![serde_json::json!({
                "instType": "spbl",
                "channel": "fill",
                "instId": Self::format_symbol(sym)
            })]
        } else {
            vec![serde_json::json!({
                "instType": "spbl",
                "channel": "fill",
                "instId": "default"
            })]
        };
        client.subscribe_private_stream(args, "fill", symbol).await
    }

    async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.as_deref().unwrap_or(""),
            self.api_secret.as_deref().unwrap_or(""),
            self.passphrase.as_deref().unwrap_or(""),
        );
        let args = if let Some(syms) = symbols {
            syms.iter().map(|sym| serde_json::json!({
                "instType": "mc",
                "channel": "positions",
                "instId": Self::format_symbol(sym)
            })).collect()
        } else {
            vec![serde_json::json!({
                "instType": "mc",
                "channel": "positions",
                "instId": "default"
            })]
        };
        client.subscribe_private_stream(args, "positions", None).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_credentials(
            self.api_key.as_deref().unwrap_or(""),
            self.api_secret.as_deref().unwrap_or(""),
            self.passphrase.as_deref().unwrap_or(""),
        );
        let args = vec![serde_json::json!({
            "instType": "spbl",
            "channel": "account",
            "coin": "default"
        })];
        client.subscribe_private_stream(args, "account", None).await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        let api_key = self.api_key.clone().ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
            message: "API key not set".into(),
        })?;
        let api_secret = self.api_secret.clone().ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
            message: "API secret not set".into(),
        })?;
        let passphrase = self.passphrase.clone().ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
            message: "Passphrase not set".into(),
        })?;

        if let Some(ref client) = self.ws_client {
            let timestamp = (Utc::now().timestamp_millis() / 1000).to_string();
            let sign = Self::generate_auth_signature(&api_secret, &timestamp);

            let login_msg = serde_json::json!({
                "op": "login",
                "args": [{
                    "apiKey": api_key,
                    "passphrase": passphrase,
                    "timestamp": timestamp,
                    "sign": sign
                }]
            });
            client.send(&login_msg.to_string())?;
        }
        Ok(())
    }
}

// === Bitget WebSocket Types ===

#[derive(Debug, Deserialize)]
struct BitgetWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    arg: Option<BitgetWsArg>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitgetWsArg {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    inst_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetTickerData {
    #[serde(default)]
    inst_id: String,
    #[serde(default)]
    last_pr: Option<String>,
    #[serde(default)]
    bid_pr: Option<String>,
    #[serde(default)]
    bid_sz: Option<String>,
    #[serde(default)]
    ask_pr: Option<String>,
    #[serde(default)]
    ask_sz: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high24h: Option<String>,
    #[serde(default)]
    low24h: Option<String>,
    #[serde(default)]
    base_volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    change: Option<String>,
    #[serde(default)]
    change_utc: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitgetOrderBookData {
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    ts: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetTradeData {
    trade_id: String,
    price: String,
    size: String,
    side: String,
    #[serde(default)]
    ts: Option<String>,
}

// === Private Channel Data Structures ===

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetOrderData {
    #[serde(default)]
    inst_id: String,
    #[serde(default)]
    ord_id: String,
    #[serde(default)]
    cl_ord_id: Option<String>,
    #[serde(default)]
    side: String,
    #[serde(default)]
    ord_type: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    px: Option<String>,
    #[serde(default)]
    sz: Option<String>,
    #[serde(default)]
    fill_sz: Option<String>,
    #[serde(default)]
    avg_px: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_ccy: Option<String>,
    #[serde(default)]
    force: Option<String>,
    #[serde(default)]
    reduce_only: Option<String>,
    #[serde(default)]
    u_time: Option<String>,
    #[serde(default)]
    c_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetFillData {
    #[serde(default)]
    inst_id: String,
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    ord_id: Option<String>,
    #[serde(default)]
    side: String,
    #[serde(default)]
    fill_px: Option<String>,
    #[serde(default)]
    fill_sz: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_ccy: Option<String>,
    #[serde(default)]
    exec_type: Option<String>,
    #[serde(default)]
    fill_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetPositionData {
    #[serde(default)]
    inst_id: String,
    #[serde(default)]
    pos_id: Option<String>,
    #[serde(default)]
    hold_side: Option<String>,
    #[serde(default)]
    margin_mode: Option<String>,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    avg_open_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    unrealized_pl: Option<String>,
    #[serde(default)]
    achieved_profits: Option<String>,
    #[serde(default)]
    leverage: Option<String>,
    #[serde(default)]
    liq_px: Option<String>,
    #[serde(default)]
    margin: Option<String>,
    #[serde(default)]
    margin_ratio: Option<String>,
    #[serde(default)]
    upl_rate: Option<String>,
    #[serde(default)]
    u_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetAccountData {
    #[serde(default)]
    details: Vec<BitgetAccountDetailData>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetAccountDetailData {
    #[serde(default)]
    coin: String,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    frozen: Option<String>,
    #[serde(default)]
    equity: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BitgetWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(BitgetWs::format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitgetWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(BitgetWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BitgetWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(BitgetWs::format_interval(Timeframe::Hour1), "1H");
        assert_eq!(BitgetWs::format_interval(Timeframe::Day1), "1D");
    }

    #[test]
    fn test_with_credentials() {
        let client = BitgetWs::with_credentials("test_key", "test_secret", "test_passphrase");
        assert_eq!(client.get_api_key(), Some("test_key"));
    }

    #[test]
    fn test_set_credentials() {
        let mut client = BitgetWs::new();
        assert_eq!(client.get_api_key(), None);
        client.set_credentials("my_key", "my_secret", "my_passphrase");
        assert_eq!(client.get_api_key(), Some("my_key"));
    }

    #[test]
    fn test_generate_auth_signature() {
        // Test signature generation
        let signature = BitgetWs::generate_auth_signature("test_secret", "1234567890");
        assert!(!signature.is_empty());
        // Signature should be base64 encoded
        assert!(signature.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn test_process_auth_success() {
        let msg = r#"{"event":"login","code":"0","msg":"success"}"#;
        let result = BitgetWs::process_message(msg, None, None);
        assert!(result.is_some());
        assert!(matches!(result.unwrap(), WsMessage::Authenticated));
    }

    #[test]
    fn test_process_auth_failure() {
        let msg = r#"{"event":"login","code":"40001","msg":"invalid signature"}"#;
        let result = BitgetWs::process_message(msg, None, None);
        assert!(result.is_some());
        if let Some(WsMessage::Error(e)) = result {
            assert!(e.contains("Auth failed"));
        } else {
            panic!("Expected Error message");
        }
    }

    #[test]
    fn test_parse_order() {
        let data = BitgetOrderData {
            inst_id: "BTCUSDT".to_string(),
            ord_id: "order123".to_string(),
            cl_ord_id: Some("client123".to_string()),
            side: "buy".to_string(),
            ord_type: "limit".to_string(),
            status: "new".to_string(),
            px: Some("50000".to_string()),
            sz: Some("0.1".to_string()),
            fill_sz: Some("0.05".to_string()),
            avg_px: Some("49990".to_string()),
            fee: Some("-0.01".to_string()),
            fee_ccy: Some("USDT".to_string()),
            force: Some("gtc".to_string()),
            reduce_only: None,
            u_time: Some("1704067200000".to_string()),
            c_time: Some("1704067100000".to_string()),
        };
        let event = BitgetWs::parse_order(&data);
        assert_eq!(event.order.id, "order123");
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
    }

    #[test]
    fn test_process_order_message() {
        let msg = r#"{
            "arg": {"channel": "orders", "instId": "BTCUSDT"},
            "data": [{
                "instId": "BTCUSDT",
                "ordId": "ord456",
                "side": "sell",
                "ordType": "market",
                "status": "filled",
                "sz": "0.5"
            }]
        }"#;
        let result = BitgetWs::process_message(msg, None, None);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "ord456");
            assert_eq!(event.order.side, OrderSide::Sell);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_parse_fill() {
        let data = BitgetFillData {
            inst_id: "ETHUSDT".to_string(),
            trade_id: Some("trade789".to_string()),
            ord_id: Some("order456".to_string()),
            side: "buy".to_string(),
            fill_px: Some("3000".to_string()),
            fill_sz: Some("1.5".to_string()),
            fee: Some("-0.005".to_string()),
            fee_ccy: Some("ETH".to_string()),
            exec_type: Some("maker".to_string()),
            fill_time: Some("1704067200000".to_string()),
        };
        let event = BitgetWs::parse_fill(&data);
        assert_eq!(event.symbol, "ETH/USDT");
        assert_eq!(event.trades.len(), 1);
        assert_eq!(event.trades[0].id, "trade789");
        assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Maker));
    }

    #[test]
    fn test_process_fill_message() {
        let msg = r#"{
            "arg": {"channel": "fill", "instId": "BTCUSDT"},
            "data": [{
                "instId": "BTCUSDT",
                "tradeId": "fill123",
                "ordId": "ord789",
                "side": "buy",
                "fillPx": "51000",
                "fillSz": "0.2",
                "execType": "taker"
            }]
        }"#;
        let result = BitgetWs::process_message(msg, None, None);
        assert!(result.is_some());
        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.trades[0].id, "fill123");
            assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Taker));
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_parse_position() {
        let data = BitgetPositionData {
            inst_id: "BTCUSDT".to_string(),
            pos_id: Some("pos123".to_string()),
            hold_side: Some("long".to_string()),
            margin_mode: Some("cross".to_string()),
            total: Some("1.0".to_string()),
            available: Some("0.8".to_string()),
            avg_open_price: Some("45000".to_string()),
            mark_price: Some("50000".to_string()),
            unrealized_pl: Some("5000".to_string()),
            achieved_profits: Some("1000".to_string()),
            leverage: Some("10".to_string()),
            liq_px: Some("40000".to_string()),
            margin: Some("4500".to_string()),
            margin_ratio: Some("0.1".to_string()),
            upl_rate: Some("0.11".to_string()),
            u_time: Some("1704067200000".to_string()),
        };
        let event = BitgetWs::parse_position(&data);
        assert_eq!(event.positions.len(), 1);
        assert_eq!(event.positions[0].symbol, "BTC/USDT");
        assert_eq!(event.positions[0].side, Some(PositionSide::Long));
        assert_eq!(event.positions[0].margin_mode, Some(MarginMode::Cross));
    }

    #[test]
    fn test_process_position_message() {
        let msg = r#"{
            "arg": {"channel": "positions", "instId": "BTCUSDT"},
            "data": [{
                "instId": "BTCUSDT",
                "posId": "pos456",
                "holdSide": "short",
                "marginMode": "isolated",
                "total": "2.0"
            }]
        }"#;
        let result = BitgetWs::process_message(msg, None, None);
        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert_eq!(event.positions[0].side, Some(PositionSide::Short));
            assert_eq!(event.positions[0].margin_mode, Some(MarginMode::Isolated));
        } else {
            panic!("Expected Position message");
        }
    }

    #[test]
    fn test_parse_account() {
        let data = BitgetAccountData {
            details: vec![
                BitgetAccountDetailData {
                    coin: "USDT".to_string(),
                    available: Some("10000".to_string()),
                    frozen: Some("500".to_string()),
                    equity: Some("10500".to_string()),
                },
                BitgetAccountDetailData {
                    coin: "BTC".to_string(),
                    available: Some("1.5".to_string()),
                    frozen: Some("0.1".to_string()),
                    equity: Some("1.6".to_string()),
                },
            ],
        };
        let event = BitgetWs::parse_account(&data);
        assert_eq!(event.balances.currencies.len(), 2);
        assert!(event.balances.currencies.contains_key("USDT"));
        assert!(event.balances.currencies.contains_key("BTC"));
    }

    #[test]
    fn test_process_account_message() {
        let msg = r#"{
            "arg": {"channel": "account"},
            "data": [{
                "details": [{
                    "coin": "ETH",
                    "available": "5.0",
                    "frozen": "0.5"
                }]
            }]
        }"#;
        let result = BitgetWs::process_message(msg, None, None);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("ETH"));
        } else {
            panic!("Expected Balance message");
        }
    }
}
