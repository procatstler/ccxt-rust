//! OKX WebSocket Implementation
//!
//! OKX 실시간 데이터 스트리밍

#![allow(clippy::manual_strip)]

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

const WS_PUBLIC_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";
const WS_PRIVATE_URL: &str = "wss://ws.okx.com:8443/ws/v5/private";

/// OKX WebSocket 클라이언트
pub struct OkxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
}

impl OkxWs {
    /// 새 OKX WebSocket 클라이언트 생성
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

    /// API 인증 정보를 포함하여 생성
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

    /// API 인증 정보 설정
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str, passphrase: &str) {
        self.api_key = Some(api_key.to_string());
        self.api_secret = Some(api_secret.to_string());
        self.passphrase = Some(passphrase.to_string());
    }

    /// API 키 반환
    pub fn get_api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// 인증 서명 생성
    /// sign = Base64(HMAC_SHA256(timestamp + method + requestPath, secretKey))
    fn generate_auth_signature(api_secret: &str, timestamp: &str) -> String {
        let sign_str = format!("{}GET/users/self/verify", timestamp);
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_str.as_bytes());
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, mac.finalize().into_bytes())
    }

    /// 심볼을 OKX 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Timeframe을 OKX bar로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1H",
            Timeframe::Hour2 => "2H",
            Timeframe::Hour4 => "4H",
            Timeframe::Hour6 => "6H",
            Timeframe::Hour12 => "12H",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            Timeframe::Month1 => "1M",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &OkxTickerData) -> WsTickerEvent {
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
            bid: data.bid_px.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_sz.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_px.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_sz.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open24h.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vol_ccy24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &OkxOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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
            nonce: data.seq_id,
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
    fn parse_trade(data: &OkxTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.px.parse().unwrap_or_default();
        let amount: Decimal = data.sz.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone(),
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

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// OKX 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(okx_symbol: &str) -> String {
        okx_symbol.replace("-", "/")
    }

    /// 주문 메시지 파싱
    fn parse_order(data: &OkxOrderData) -> WsOrderEvent {
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
            "post_only" => OrderType::Limit,
            "fok" => OrderType::Limit,
            "ioc" => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let status = match data.state.to_lowercase().as_str() {
            "live" => OrderStatus::Open,
            "partially_filled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "canceled" => OrderStatus::Canceled,
            "cancelled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let time_in_force = match data.ord_type.to_lowercase().as_str() {
            "fok" => TimeInForce::FOK,
            "ioc" => TimeInForce::IOC,
            "post_only" => TimeInForce::PO,
            _ => TimeInForce::GTC,
        };

        let price: Option<Decimal> = data.px.as_ref().and_then(|p| p.parse().ok());
        let amount: Option<Decimal> = data.sz.as_ref().and_then(|s| s.parse().ok());
        let filled: Option<Decimal> = data.fill_sz.as_ref().and_then(|f| f.parse().ok());
        let avg_price: Option<Decimal> = data.avg_px.as_ref().and_then(|a| a.parse().ok());

        let cost = match (filled, avg_price) {
            (Some(f), Some(a)) => Some(f * a),
            _ => None,
        };

        let remaining = match (amount, filled) {
            (Some(a), Some(f)) => Some(a - f),
            (Some(a), None) => Some(a),
            _ => None,
        };

        // Parse fee
        let mut fees = Vec::new();
        let fee = if let (Some(fee_str), Some(fee_ccy)) = (&data.fee, &data.fee_ccy) {
            if let Ok(fee_val) = fee_str.parse::<Decimal>() {
                let f = Fee {
                    cost: Some(fee_val.abs()),
                    currency: Some(fee_ccy.clone()),
                    rate: None,
                };
                fees.push(f.clone());
                Some(f)
            } else {
                None
            }
        } else {
            None
        };

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
            symbol: symbol.clone(),
            order_type,
            time_in_force: Some(time_in_force),
            side,
            price,
            average: avg_price,
            amount: amount.unwrap_or_default(),
            filled: filled.unwrap_or_default(),
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            reduce_only: data.reduce_only.as_ref().map(|r| r == "true"),
            post_only: Some(data.ord_type.to_lowercase() == "post_only"),
            trades: Vec::new(),
            fee,
            fees,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// 체결 내역 파싱 (fills channel)
    fn parse_fill(data: &OkxFillData) -> WsMyTradeEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.fill_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let price: Decimal = data.fill_px.parse().unwrap_or_default();
        let amount: Decimal = data.fill_sz.parse().unwrap_or_default();

        let mut fees = Vec::new();
        let fee = if let Ok(fee_val) = data.fee.parse::<Decimal>() {
            let f = Fee {
                cost: Some(fee_val.abs()),
                currency: Some(data.fee_ccy.clone()),
                rate: None,
            };
            fees.push(f.clone());
            Some(f)
        } else {
            None
        };

        let trade = Trade {
            id: data.trade_id.clone(),
            order: Some(data.ord_id.clone()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.side.to_lowercase()),
            taker_or_maker: data.exec_type.as_ref().map(|e| {
                if e == "T" { TakerOrMaker::Taker } else { TakerOrMaker::Maker }
            }),
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
    fn parse_position(data: &OkxPositionData) -> WsPositionEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.u_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let pos_side = match data.pos_side.to_lowercase().as_str() {
            "long" => PositionSide::Long,
            "short" => PositionSide::Short,
            _ => PositionSide::Unknown,
        };

        let margin_mode = match data.mgn_mode.as_deref().unwrap_or("").to_lowercase().as_str() {
            "isolated" => MarginMode::Isolated,
            "cross" => MarginMode::Cross,
            _ => MarginMode::Unknown,
        };

        let contracts: Option<Decimal> = data.pos.as_ref().and_then(|p| p.parse().ok());
        let entry_price: Option<Decimal> = data.avg_px.as_ref().and_then(|a| a.parse().ok());
        let mark_price: Option<Decimal> = data.mark_px.as_ref().and_then(|m| m.parse().ok());
        let unrealized_pnl: Option<Decimal> = data.upl.as_ref().and_then(|u| u.parse().ok());
        let leverage: Option<Decimal> = data.lever.as_ref().and_then(|l| l.parse().ok());
        let liquidation_price: Option<Decimal> = data.liq_px.as_ref().and_then(|l| l.parse().ok());
        let margin: Option<Decimal> = data.margin.as_ref().and_then(|m| m.parse().ok());

        let position = Position {
            id: Some(data.pos_id.clone()),
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
            realized_pnl: data.realized_pnl.as_ref().and_then(|r| r.parse().ok()),
            collateral: margin,
            entry_price,
            mark_price,
            liquidation_price,
            margin_mode: Some(margin_mode),
            hedged: None,
            margin_ratio: data.mgn_ratio.as_ref().and_then(|m| m.parse().ok()),
            initial_margin: None,
            initial_margin_percentage: None,
            maintenance_margin: None,
            maintenance_margin_percentage: None,
            last_update_timestamp: Some(timestamp),
            last_price: mark_price,
            stop_loss_price: None,
            take_profit_price: None,
            percentage: data.upl_ratio.as_ref().and_then(|u| u.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsPositionEvent {
            positions: vec![position],
        }
    }

    /// 계정 잔고 메시지 파싱
    fn parse_account(data: &OkxAccountData) -> WsBalanceEvent {
        let timestamp = data.u_time.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for detail in &data.details {
            let free: Decimal = detail.avail_bal.as_ref()
                .and_then(|a| a.parse().ok())
                .unwrap_or_default();
            let used: Decimal = detail.frozen_bal.as_ref()
                .and_then(|f| f.parse().ok())
                .unwrap_or_default();
            let total = free + used;

            currencies.insert(detail.ccy.clone(), Balance {
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
    fn process_message(msg: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<OkxWsResponse>(msg) {
            // 로그인 응답 처리 (에러 체크 전에)
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

            // 에러 체크
            if response.code.as_ref().map(|c| c != "0").unwrap_or(false) {
                return Some(WsMessage::Error(response.msg.unwrap_or_default()));
            }

            // 이벤트 처리
            if let Some(event) = &response.event {
                if event == "subscribe" {
                    return Some(WsMessage::Subscribed {
                        channel: response.arg.as_ref()
                            .and_then(|a| a.channel.clone())
                            .unwrap_or_default(),
                        symbol: response.arg.as_ref().and_then(|a| a.inst_id.clone()),
                    });
                }
            }

            // 데이터 처리
            if let (Some(arg), Some(data_arr)) = (&response.arg, &response.data) {
                let channel = arg.channel.as_deref().unwrap_or("");

                // === Public Channels ===

                // Ticker
                if channel == "tickers" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ticker_data) = serde_json::from_value::<OkxTickerData>(first.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }
                }

                // OrderBook
                if channel.starts_with("books") {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ob_data) = serde_json::from_value::<OkxOrderBookData>(first.clone()) {
                            let symbol = arg.inst_id.as_ref()
                                .map(|s| Self::to_unified_symbol(s))
                                .unwrap_or_default();
                            let is_snapshot = response.action.as_deref() == Some("snapshot");
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                        }
                    }
                }

                // Trade
                if channel == "trades" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(trade_data) = serde_json::from_value::<OkxTradeData>(first.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                        }
                    }
                }

                // Candle
                if channel.starts_with("candle") {
                    if let Some(first) = data_arr.first() {
                        if let Ok(candle_arr) = serde_json::from_value::<Vec<String>>(first.clone()) {
                            let symbol = arg.inst_id.as_ref()
                                .map(|s| Self::to_unified_symbol(s))
                                .unwrap_or_default();
                            // Extract timeframe from channel (e.g., "candle1m")
                            let timeframe = match &channel[6..] {
                                "1m" => Timeframe::Minute1,
                                "3m" => Timeframe::Minute3,
                                "5m" => Timeframe::Minute5,
                                "15m" => Timeframe::Minute15,
                                "30m" => Timeframe::Minute30,
                                "1H" => Timeframe::Hour1,
                                "2H" => Timeframe::Hour2,
                                "4H" => Timeframe::Hour4,
                                "6H" => Timeframe::Hour6,
                                "12H" => Timeframe::Hour12,
                                "1D" => Timeframe::Day1,
                                "1W" => Timeframe::Week1,
                                "1M" => Timeframe::Month1,
                                _ => Timeframe::Minute1,
                            };
                            if let Some(event) = Self::parse_candle(&candle_arr, &symbol, timeframe) {
                                return Some(WsMessage::Ohlcv(event));
                            }
                        }
                    }
                }

                // === Private Channels ===

                // Orders
                if channel == "orders" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(order_data) = serde_json::from_value::<OkxOrderData>(first.clone()) {
                            return Some(WsMessage::Order(Self::parse_order(&order_data)));
                        }
                    }
                }

                // Fills (My Trades)
                if channel == "fills" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(fill_data) = serde_json::from_value::<OkxFillData>(first.clone()) {
                            return Some(WsMessage::MyTrade(Self::parse_fill(&fill_data)));
                        }
                    }
                }

                // Positions
                if channel == "positions" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(pos_data) = serde_json::from_value::<OkxPositionData>(first.clone()) {
                            return Some(WsMessage::Position(Self::parse_position(&pos_data)));
                        }
                    }
                }

                // Account (Balance)
                if channel == "account" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(account_data) = serde_json::from_value::<OkxAccountData>(first.clone()) {
                            return Some(WsMessage::Balance(Self::parse_account(&account_data)));
                        }
                    }
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, args: Vec<serde_json::Value>, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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

    /// Private 채널 구독 시작 및 이벤트 스트림 반환 (인증 필요)
    async fn subscribe_private_stream(
        &mut self,
        args: Vec<serde_json::Value>,
        channel: &str,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let api_key = self.api_key.clone().ok_or_else(|| {
            crate::errors::CcxtError::AuthenticationError {
                message: "API key required for private channels".into(),
            }
        })?;
        let api_secret = self.api_secret.clone().ok_or_else(|| {
            crate::errors::CcxtError::AuthenticationError {
                message: "API secret required for private channels".into(),
            }
        })?;
        let passphrase = self.passphrase.clone().ok_or_else(|| {
            crate::errors::CcxtError::AuthenticationError {
                message: "Passphrase required for private channels".into(),
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

        // 인증 메시지 전송
        let timestamp = (Utc::now().timestamp_millis() / 1000).to_string();
        let sign = Self::generate_auth_signature(&api_secret, &timestamp);

        let auth_msg = serde_json::json!({
            "op": "login",
            "args": [{
                "apiKey": api_key,
                "passphrase": passphrase,
                "timestamp": timestamp,
                "sign": sign
            }]
        });
        ws_client.send(&auth_msg.to_string())?;

        // 구독 메시지 전송 (인증 후 바로 전송)
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

impl Default for OkxWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for OkxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args = vec![serde_json::json!({
            "channel": "tickers",
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args: Vec<serde_json::Value> = symbols
            .iter()
            .map(|s| serde_json::json!({
                "channel": "tickers",
                "instId": Self::format_symbol(s)
            }))
            .collect();
        client.subscribe_stream(args, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = match limit.unwrap_or(400) {
            1 => "bbo-tbt",
            5 => "books5",
            _ => "books",
        };
        let args = vec![serde_json::json!({
            "channel": channel,
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args = vec![serde_json::json!({
            "channel": "trades",
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let args = vec![serde_json::json!({
            "channel": format!("candle{}", interval),
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "ohlcv", Some(symbol)).await
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
        let mut client = OkxWs {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            passphrase: self.passphrase.clone(),
        };

        let args = match symbol {
            Some(s) => vec![serde_json::json!({
                "channel": "orders",
                "instType": "ANY",
                "instId": Self::format_symbol(s)
            })],
            None => vec![serde_json::json!({
                "channel": "orders",
                "instType": "ANY"
            })],
        };
        client.subscribe_private_stream(args, "orders", symbol).await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = OkxWs {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            passphrase: self.passphrase.clone(),
        };

        let args = match symbol {
            Some(s) => vec![serde_json::json!({
                "channel": "fills",
                "instType": "ANY",
                "instId": Self::format_symbol(s)
            })],
            None => vec![serde_json::json!({
                "channel": "fills",
                "instType": "ANY"
            })],
        };
        client.subscribe_private_stream(args, "fills", symbol).await
    }

    async fn watch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = OkxWs {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            passphrase: self.passphrase.clone(),
        };

        let args = match symbols {
            Some(syms) if !syms.is_empty() => {
                syms.iter().map(|s| serde_json::json!({
                    "channel": "positions",
                    "instType": "ANY",
                    "instId": Self::format_symbol(s)
                })).collect()
            }
            _ => vec![serde_json::json!({
                "channel": "positions",
                "instType": "ANY"
            })],
        };
        client.subscribe_private_stream(args, "positions", None).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = OkxWs {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            passphrase: self.passphrase.clone(),
        };

        let args = vec![serde_json::json!({
            "channel": "account"
        })];
        client.subscribe_private_stream(args, "account", None).await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        let api_key = self.api_key.as_ref().ok_or_else(|| {
            crate::errors::CcxtError::AuthenticationError {
                message: "API key required for authentication".into(),
            }
        })?;
        let api_secret = self.api_secret.as_ref().ok_or_else(|| {
            crate::errors::CcxtError::AuthenticationError {
                message: "API secret required for authentication".into(),
            }
        })?;
        let passphrase = self.passphrase.as_ref().ok_or_else(|| {
            crate::errors::CcxtError::AuthenticationError {
                message: "Passphrase required for authentication".into(),
            }
        })?;

        if let Some(client) = &self.ws_client {
            let timestamp = (Utc::now().timestamp_millis() / 1000).to_string();
            let sign = Self::generate_auth_signature(api_secret, &timestamp);

            let auth_msg = serde_json::json!({
                "op": "login",
                "args": [{
                    "apiKey": api_key,
                    "passphrase": passphrase,
                    "timestamp": timestamp,
                    "sign": sign
                }]
            });
            client.send(&auth_msg.to_string())?;
        }

        Ok(())
    }
}

// === OKX WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct OkxWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    arg: Option<OkxWsArg>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxWsArg {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    inst_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxTickerData {
    inst_id: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    last_sz: Option<String>,
    #[serde(default)]
    ask_px: Option<String>,
    #[serde(default)]
    ask_sz: Option<String>,
    #[serde(default)]
    bid_px: Option<String>,
    #[serde(default)]
    bid_sz: Option<String>,
    #[serde(default)]
    open24h: Option<String>,
    #[serde(default)]
    high24h: Option<String>,
    #[serde(default)]
    low24h: Option<String>,
    #[serde(default)]
    vol_ccy24h: Option<String>,
    #[serde(default)]
    vol24h: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxOrderBookData {
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    seq_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxTradeData {
    inst_id: String,
    trade_id: String,
    px: String,
    sz: String,
    side: String,
    #[serde(default)]
    ts: Option<String>,
}

// === Private Channel Data Types ===

/// OKX 주문 데이터 (orders channel)
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxOrderData {
    inst_id: String,
    ord_id: String,
    #[serde(default)]
    cl_ord_id: Option<String>,
    side: String,
    ord_type: String,
    state: String,
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
    reduce_only: Option<String>,
    #[serde(default)]
    u_time: Option<String>,
    #[serde(default)]
    c_time: Option<String>,
}

/// OKX 체결 데이터 (fills channel)
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxFillData {
    inst_id: String,
    trade_id: String,
    ord_id: String,
    #[serde(default)]
    cl_ord_id: Option<String>,
    side: String,
    fill_px: String,
    fill_sz: String,
    fee: String,
    fee_ccy: String,
    #[serde(default)]
    exec_type: Option<String>,
    #[serde(default)]
    fill_time: Option<String>,
}

/// OKX 포지션 데이터 (positions channel)
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxPositionData {
    inst_id: String,
    pos_id: String,
    pos_side: String,
    #[serde(default)]
    pos: Option<String>,
    #[serde(default)]
    avg_px: Option<String>,
    #[serde(default)]
    mark_px: Option<String>,
    #[serde(default)]
    upl: Option<String>,
    #[serde(default)]
    upl_ratio: Option<String>,
    #[serde(default)]
    realized_pnl: Option<String>,
    #[serde(default)]
    lever: Option<String>,
    #[serde(default)]
    liq_px: Option<String>,
    #[serde(default)]
    margin: Option<String>,
    #[serde(default)]
    mgn_mode: Option<String>,
    #[serde(default)]
    mgn_ratio: Option<String>,
    #[serde(default)]
    u_time: Option<String>,
}

/// OKX 계정 데이터 (account channel)
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxAccountData {
    #[serde(default)]
    u_time: Option<String>,
    #[serde(default)]
    details: Vec<OkxAccountDetailData>,
}

/// OKX 계정 상세 데이터
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxAccountDetailData {
    ccy: String,
    #[serde(default)]
    avail_bal: Option<String>,
    #[serde(default)]
    frozen_bal: Option<String>,
    #[serde(default)]
    eq: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(OkxWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(OkxWs::format_symbol("ETH/BTC"), "ETH-BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(OkxWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(OkxWs::to_unified_symbol("ETH-BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(OkxWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(OkxWs::format_interval(Timeframe::Hour1), "1H");
        assert_eq!(OkxWs::format_interval(Timeframe::Day1), "1D");
    }

    #[test]
    fn test_with_credentials() {
        let client = OkxWs::with_credentials("api_key", "api_secret", "passphrase");
        assert_eq!(client.api_key, Some("api_key".to_string()));
        assert_eq!(client.api_secret, Some("api_secret".to_string()));
        assert_eq!(client.passphrase, Some("passphrase".to_string()));
    }

    #[test]
    fn test_set_credentials() {
        let mut client = OkxWs::new();
        client.set_credentials("api_key", "api_secret", "passphrase");
        assert_eq!(client.api_key, Some("api_key".to_string()));
        assert_eq!(client.api_secret, Some("api_secret".to_string()));
        assert_eq!(client.passphrase, Some("passphrase".to_string()));
    }

    #[test]
    fn test_generate_auth_signature() {
        let secret = "test_secret";
        let timestamp = "1704067200";
        let sign = OkxWs::generate_auth_signature(secret, timestamp);
        assert!(!sign.is_empty());
        // Base64 encoded signature
        assert!(sign.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn test_parse_order() {
        let data = OkxOrderData {
            inst_id: "BTC-USDT".to_string(),
            ord_id: "order123".to_string(),
            cl_ord_id: Some("client123".to_string()),
            side: "buy".to_string(),
            ord_type: "limit".to_string(),
            state: "live".to_string(),
            px: Some("50000".to_string()),
            sz: Some("0.1".to_string()),
            fill_sz: Some("0.05".to_string()),
            avg_px: Some("49500".to_string()),
            fee: Some("-0.0001".to_string()),
            fee_ccy: Some("BTC".to_string()),
            reduce_only: None,
            u_time: Some("1704067200000".to_string()),
            c_time: None,
        };

        let event = OkxWs::parse_order(&data);
        assert_eq!(event.order.id, "order123");
        assert_eq!(event.order.symbol, "BTC/USDT");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
    }

    #[test]
    fn test_parse_fill() {
        let data = OkxFillData {
            inst_id: "BTC-USDT".to_string(),
            trade_id: "trade123".to_string(),
            ord_id: "order123".to_string(),
            cl_ord_id: None,
            side: "buy".to_string(),
            fill_px: "50000".to_string(),
            fill_sz: "0.1".to_string(),
            fee: "-0.0001".to_string(),
            fee_ccy: "BTC".to_string(),
            exec_type: Some("T".to_string()),
            fill_time: Some("1704067200000".to_string()),
        };

        let event = OkxWs::parse_fill(&data);
        assert_eq!(event.symbol, "BTC/USDT");
        assert_eq!(event.trades.len(), 1);
        assert_eq!(event.trades[0].id, "trade123");
        assert_eq!(event.trades[0].order, Some("order123".to_string()));
    }

    #[test]
    fn test_parse_position() {
        let data = OkxPositionData {
            inst_id: "BTC-USDT-SWAP".to_string(),
            pos_id: "pos123".to_string(),
            pos_side: "long".to_string(),
            pos: Some("1".to_string()),
            avg_px: Some("50000".to_string()),
            mark_px: Some("51000".to_string()),
            upl: Some("1000".to_string()),
            upl_ratio: Some("0.02".to_string()),
            realized_pnl: Some("500".to_string()),
            lever: Some("10".to_string()),
            liq_px: Some("45000".to_string()),
            margin: Some("5000".to_string()),
            mgn_mode: Some("cross".to_string()),
            mgn_ratio: Some("0.1".to_string()),
            u_time: Some("1704067200000".to_string()),
        };

        let event = OkxWs::parse_position(&data);
        assert_eq!(event.positions.len(), 1);
        assert_eq!(event.positions[0].symbol, "BTC/USDT/SWAP");
        assert_eq!(event.positions[0].side, Some(PositionSide::Long));
    }

    #[test]
    fn test_parse_account() {
        let data = OkxAccountData {
            u_time: Some("1704067200000".to_string()),
            details: vec![
                OkxAccountDetailData {
                    ccy: "BTC".to_string(),
                    avail_bal: Some("1.5".to_string()),
                    frozen_bal: Some("0.5".to_string()),
                    eq: Some("2.0".to_string()),
                },
                OkxAccountDetailData {
                    ccy: "USDT".to_string(),
                    avail_bal: Some("10000".to_string()),
                    frozen_bal: Some("5000".to_string()),
                    eq: Some("15000".to_string()),
                },
            ],
        };

        let event = OkxWs::parse_account(&data);
        assert_eq!(event.balances.currencies.len(), 2);
        assert!(event.balances.currencies.contains_key("BTC"));
        assert!(event.balances.currencies.contains_key("USDT"));
    }

    #[test]
    fn test_process_order_message() {
        let msg = r#"{
            "arg": {"channel": "orders", "instType": "ANY"},
            "data": [{
                "instId": "BTC-USDT",
                "ordId": "order123",
                "side": "buy",
                "ordType": "limit",
                "state": "filled",
                "px": "50000",
                "sz": "0.1",
                "fillSz": "0.1",
                "avgPx": "50000",
                "uTime": "1704067200000"
            }]
        }"#;

        let result = OkxWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "order123");
            assert_eq!(event.order.status, OrderStatus::Closed);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_fill_message() {
        let msg = r#"{
            "arg": {"channel": "fills", "instType": "ANY"},
            "data": [{
                "instId": "BTC-USDT",
                "tradeId": "trade123",
                "ordId": "order123",
                "side": "sell",
                "fillPx": "51000",
                "fillSz": "0.1",
                "fee": "-0.0001",
                "feeCcy": "BTC",
                "execType": "M",
                "fillTime": "1704067200000"
            }]
        }"#;

        let result = OkxWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.trades[0].id, "trade123");
            assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Maker));
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_process_position_message() {
        let msg = r#"{
            "arg": {"channel": "positions", "instType": "ANY"},
            "data": [{
                "instId": "BTC-USDT-SWAP",
                "posId": "pos123",
                "posSide": "net",
                "pos": "2",
                "avgPx": "50000",
                "uTime": "1704067200000"
            }]
        }"#;

        let result = OkxWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert_eq!(event.positions[0].id, Some("pos123".to_string()));
        } else {
            panic!("Expected Position message");
        }
    }

    #[test]
    fn test_process_account_message() {
        let msg = r#"{
            "arg": {"channel": "account"},
            "data": [{
                "uTime": "1704067200000",
                "details": [
                    {"ccy": "BTC", "availBal": "1.0", "frozenBal": "0.5"}
                ]
            }]
        }"#;

        let result = OkxWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_auth_success() {
        let msg = r#"{"event": "login", "code": "0", "msg": ""}"#;

        let result = OkxWs::process_message(msg);
        assert!(result.is_some());
        assert!(matches!(result, Some(WsMessage::Authenticated)));
    }

    #[test]
    fn test_process_auth_failure() {
        let msg = r#"{"event": "login", "code": "60001", "msg": "Invalid credentials"}"#;

        let result = OkxWs::process_message(msg);
        assert!(result.is_some());
        if let Some(WsMessage::Error(e)) = result {
            assert!(e.contains("Auth failed"));
        } else {
            panic!("Expected Error message");
        }
    }
}
