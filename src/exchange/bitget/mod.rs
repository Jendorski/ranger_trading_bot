use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use log::{error, info};
use reqwest::Client;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use crate::{
    bot::{OpenPosition, Position},
    config::Config,
    encryption,
    helper::Helper,
};

pub mod fees;

//For binance: https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=100
//FOR BITGET, USE: https://api.bitget.com/api/v2/public/time to get the Bitget Server time

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub code: String,
    pub msg: String,
    #[serde(rename = "requestTime")]
    pub request_time: i64,
    pub data: T,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Candle {
    #[serde(deserialize_with = "deserialize_string_to_i64")]
    pub timestamp: i64,
    #[serde(deserialize_with = "deserialize_string_to_f64")]
    pub open: f64,
    #[serde(deserialize_with = "deserialize_string_to_f64")]
    pub high: f64,
    #[serde(deserialize_with = "deserialize_string_to_f64")]
    pub low: f64,
    #[serde(deserialize_with = "deserialize_string_to_f64")]
    pub close: f64,
    #[serde(deserialize_with = "deserialize_string_to_f64")]
    pub volume: f64,
    #[serde(deserialize_with = "deserialize_string_to_f64")]
    pub quote_volume: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlaceOrderData {
    #[serde(rename = "clientOid")]
    pub client_oid: String,
    #[serde(rename = "orderId")]
    pub order_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FundingRateData {
    pub symbol: String,
    #[serde(rename = "fundingRate")]
    pub funding_rate: String,
    #[serde(rename = "fundingTime")]
    pub funding_time: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderDetail {
    pub symbol: String,
    pub size: String,
    #[serde(rename = "orderId")]
    pub order_id: String,
    #[serde(rename = "clientOid")]
    pub client_oid: String,
    #[serde(rename = "baseVolume")]
    pub base_volume: String,
    #[serde(rename = "priceAvg")]
    pub price_avg: String,
    pub fee: String,
    pub price: String,
    pub state: String,
    pub side: String,
    pub force: String,
    #[serde(rename = "totalProfits")]
    pub total_profits: String,
    #[serde(rename = "posSide")]
    pub pos_side: String,
    #[serde(rename = "marginCoin")]
    pub margin_coin: String,
    #[serde(rename = "presetStopSurplusPrice")]
    pub preset_stop_surplus_price: String,
    #[serde(rename = "presetStopSurplusType")]
    pub preset_stop_surplus_type: String,
    #[serde(rename = "presetStopSurplusExecutePrice")]
    pub preset_stop_surplus_execute_price: String,
    #[serde(rename = "presetStopLossPrice")]
    pub preset_stop_loss_price: String,
    #[serde(rename = "presetStopLossType")]
    pub preset_stop_loss_type: String,
    #[serde(rename = "presetStopLossExecutePrice")]
    pub preset_stop_loss_execute_price: String,
    #[serde(rename = "quoteVolume")]
    pub quote_volume: String,
    #[serde(rename = "orderType")]
    pub order_type: String,
    pub leverage: String,
    #[serde(rename = "marginMode")]
    pub margin_mode: String,
    #[serde(rename = "reduceOnly")]
    pub reduce_only: String,
    #[serde(rename = "enterPointSource")]
    pub enter_point_source: String,
    #[serde(rename = "tradeSide")]
    pub trade_side: String,
    #[serde(rename = "posMode")]
    pub pos_mode: String,
    #[serde(rename = "orderSource")]
    pub order_source: String,
    #[serde(rename = "cancelReason")]
    pub cancel_reason: String,
    #[serde(rename = "cTime")]
    pub c_time: String,
    #[serde(rename = "uTime")]
    pub u_time: String,
}

// Custom deserializers for string-to-number conversion
pub(crate) fn deserialize_string_to_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse::<i64>().map_err(serde::de::Error::custom)
}

pub(crate) fn deserialize_string_to_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse::<f64>().map_err(serde::de::Error::custom)
}

#[async_trait]
pub trait CandleData: Send + Sync {
    fn new() -> Self;

    /// Return the latest candles
    async fn get_bitget_candles(&self, interval: String, limit: String) -> Result<Vec<Candle>>;

    /// Return the historical funding rates
    async fn get_history_funding_rate(&self, limit: String) -> Result<Vec<FundingRateData>>;
}

//#[async_trait]
pub trait FuturesCall {
    fn new() -> Self;

    /// Return the latest candles
    async fn new_futures_call(&self, open_position: OpenPosition) -> Result<PlaceOrderData>;

    async fn modify_futures_order(&self, open_position: OpenPosition) -> Result<PlaceOrderData>;
}

/// Simple HTTP‑based mock of the `Exchange` trait – replace with your real SDK.
///
/// In this example we hit a public ticker endpoint (e.g. Binance).
pub struct HttpCandleData {
    pub client: reqwest::Client,
    pub(crate) symbol: String,
    config: Config,
}

#[async_trait::async_trait]
impl CandleData for HttpCandleData {
    fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            symbol: String::from("BTCUSDT"),
            config: Config::from_env().unwrap(),
        }
    }

    async fn get_bitget_candles(&self, interval: String, limit: String) -> Result<Vec<Candle>> {
        let url = format!(
            "https://api.bitget.com/api/v2/mix/market/candles?symbol={}&granularity={}&limit={}&productType=usdt-futures",
            self.symbol, interval, limit
        );
        //info!("url: {:?}", url);
        let bitget = self.client.get(url).send().await?;

        let bit_text = bitget.text().await?;

        let response: ApiResponse<Vec<Candle>> = serde_json::from_str(&bit_text).unwrap();
        assert_eq!(response.code, "00000");
        assert_eq!(response.msg, "success");
        // assert_eq!(response.data.len(), limit.parse::<usize>().unwrap());

        let candles = response.data;

        Ok(candles)
    }

    async fn get_history_funding_rate(&self, limit: String) -> Result<Vec<FundingRateData>> {
        let url = format!(
            "https://api.bitget.com/api/v2/mix/market/history-fund-rate?symbol={}&productType=usdt-futures&limit={}",
            self.symbol, limit
        );

        let response = self.client.get(url).send().await?;
        let text = response.text().await?;
        let api_response: ApiResponse<Vec<FundingRateData>> = serde_json::from_str(&text)?;

        if api_response.code != "00000" {
            return Err(anyhow::anyhow!("Bitget API error: {}", api_response.msg));
        }

        Ok(api_response.data)
    }
}

//#[async_trait::async_trait]
impl FuturesCall for HttpCandleData {
    fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            symbol: String::new(),
            config: Config::from_env().unwrap(),
        }
    }

    async fn modify_futures_order(&self, open_position: OpenPosition) -> Result<PlaceOrderData> {
        let api_key = &self.config.api_key;
        let secret = &self.config.api_secret;
        let passphrase = &self.config.passphrase;

        let base_url = "https://api.bitget.com";
        let path = "/api/v2/mix/order/place-order";
        let method = "POST";

        let size = open_position.position_size.to_string();

        let price = open_position.entry_price.to_string();

        let client_order_id = open_position.id.to_string();

        let mut side: &str = "sell";

        if open_position.pos == Position::Long {
            side = "sell";
        }

        if open_position.pos == Position::Short {
            side = "buy";
        }

        let body_json = json!({
            "symbol": "BTCUSDT",
            "side": side,
            "orderType": "market",
            "tradeSide": "close",
            "size": size,
            "price": price,
            "marginMode": "isolated",
            "timeInForce": "goodTillCancel",
            "productType": "USDT-FUTURES",
            "marginCoin": "USDT",
            "force": "gtc",
            "clientOid": client_order_id
        });

        let body = body_json.to_string();

        let timestamp = Utc::now().timestamp_millis().to_string();

        let sign = encryption::bitget_sign(&secret, &timestamp, method, path, None, Some(&body));

        let client = Client::new();
        let response = client
            .post(format!("{base_url}{path}"))
            .header("ACCESS-KEY", api_key)
            .header("ACCESS-SIGN", sign)
            .header("ACCESS-TIMESTAMP", &timestamp)
            .header("ACCESS-PASSPHRASE", passphrase)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?;
        let response_txt = response.text().await?;
        info!("response_txt: {:?}", response_txt);

        let response: ApiResponse<PlaceOrderData> =
            serde_json::from_str(&response_txt).unwrap_or(ApiResponse {
                code: "4000".to_string(),
                msg: "Error".to_string(),
                request_time: Utc::now().timestamp(),
                data: PlaceOrderData {
                    client_oid: String::from("Failed to modify order"),
                    order_id: String::from("Failed to modify order"),
                },
            });
        info!("response::modify_futures_order -> {:?}", response);

        if response.code != "00000" {
            return Ok(PlaceOrderData {
                client_oid: String::from("Failed to modify order"),
                order_id: String::from("Failed to modify order"),
            });
        }

        let order_data = response.data;

        Ok(order_data)
    }

    async fn new_futures_call(&self, open_position: OpenPosition) -> Result<PlaceOrderData> {
        let api_key = &self.config.api_key;
        let secret = &self.config.api_secret;
        let passphrase = &self.config.passphrase;

        let base_url = "https://api.bitget.com";
        let path = "/api/v2/mix/order/place-order";
        let method = "POST";

        //let preset_stop_surplus_price = open_position.tp.unwrap().to_string();
        let f64_sl = Helper::decimal_to_f64(open_position.sl.unwrap_or(dec!(0.00)));
        let preset_stop_loss_price = Helper::truncate_to_1_dp(f64_sl);

        let size = open_position.position_size.to_string();

        let price = open_position.entry_price.to_string();

        let client_order_id = open_position.id.to_string();

        let mut side: &str = "buy";

        if open_position.pos == Position::Long {
            side = "buy";
        }

        if open_position.pos == Position::Short {
            side = "sell";
        }

        let body_json = json!({
            "symbol": "BTCUSDT",
            "side": side,
            "orderType": "market",
            "size": size,
            "price": price,
            "marginMode": "isolated",
            "timeInForce": "goodTillCancel",
            "productType": "USDT-FUTURES",
            "marginCoin": "USDT",
            "force": "gtc",
            "clientOid": client_order_id,
            //"presetStopSurplusPrice": preset_stop_surplus_price,
            "presetStopLossPrice": preset_stop_loss_price
        });

        let body = body_json.to_string();

        let timestamp = Utc::now().timestamp_millis().to_string();

        let sign = encryption::bitget_sign(&secret, &timestamp, method, path, None, Some(&body));

        let client = Client::new();
        let response = client
            .post(format!("{base_url}{path}"))
            .header("ACCESS-KEY", api_key)
            .header("ACCESS-SIGN", sign)
            .header("ACCESS-TIMESTAMP", &timestamp)
            .header("ACCESS-PASSPHRASE", passphrase)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?;
        let response_txt = response.text().await?;
        info!("response_txt: {:?}", response_txt);

        let response_json: ApiResponse<PlaceOrderData> = serde_json::from_str(&response_txt)
            .unwrap_or(ApiResponse {
                code: "4000".to_string(),
                msg: "An error occurred".to_string(),
                request_time: Utc::now().timestamp(),
                data: PlaceOrderData {
                    client_oid: String::from("Failed to place order"),
                    order_id: String::from("Failed to place order"),
                },
            }); //.expect("An error occurred");

        if response_json.code != "00000" {
            return Ok(PlaceOrderData {
                client_oid: String::from("Failed to place order"),
                order_id: String::from("Failed to place order"),
            });
        }

        let order = response_json.data;

        Ok(order)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PriceData {
    pub symbol: String,
    pub price: String,
    #[serde(rename = "indexPrice")]
    pub index_price: String,
    #[serde(rename = "markPrice")]
    pub mark_price: String,
    pub ts: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PriceResponse {
    pub code: String,
    pub msg: String,
    #[serde(rename = "requestTime")]
    pub request_time: i64,
    pub data: Vec<PriceData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prices {
    pub price: f64,
    #[serde(rename = "indexPrice")]
    pub index_price: f64,
    #[serde(rename = "markPrice")]
    pub mark_price: f64,
}

pub fn parse_price_response(json: &str) -> Result<Vec<Prices>> {
    let response: PriceResponse = serde_json::from_str::<PriceResponse>(&json)?;

    let prices = response
        .data
        .into_iter()
        .map(|item| Prices {
            price: item.price.parse().unwrap_or(1.11),
            index_price: item.index_price.parse().unwrap_or(1.11),
            mark_price: item.mark_price.parse().unwrap_or(1.11),
        })
        .collect();

    Ok(prices)
}

pub fn get_prices(json: &str) -> Option<Prices> {
    return parse_price_response(json)
        .ok()
        .and_then(|mut prices| prices.pop());
}

// WebSocket Tickers Channel Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsTickerResponse {
    pub action: String,
    pub arg: WsTickerArg,
    pub data: Vec<WsTickerData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsTickerArg {
    pub inst_type: String,
    pub channel: String,
    pub inst_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsTickerData {
    pub inst_id: String,
    pub last_pr: String,
    pub bid_pr: String,
    pub ask_pr: String,
    pub bid_sz: String,
    pub ask_sz: String,
    pub high24h: String,
    pub low24h: String,
    pub base_volume: String,
    pub quote_volume: String,
    pub open_utc: String,
    pub symbol_type: String,
    pub symbol: String,
    pub ts: String,
}

// WebSocket Candlesticks Channel Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsCandleResponse {
    pub action: String,
    pub arg: WsCandleArg,
    pub data: Vec<WsCandleData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsCandleArg {
    pub inst_type: String,
    pub channel: String,
    pub inst_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsCandleData {
    #[serde(rename = "ts")]
    pub timestamp: String,
    #[serde(rename = "o")]
    pub open: String,
    #[serde(rename = "h")]
    pub high: String,
    #[serde(rename = "l")]
    pub low: String,
    #[serde(rename = "c")]
    pub close: String,
    #[serde(rename = "baseVol")]
    pub base_volume: String,
    #[serde(rename = "quoteVol")]
    pub quote_volume: String,
}

/// Converts user-friendly timeframe to Bitget channel name
///
/// # Examples
/// - "1m" -> "candle1m"
/// - "5m" -> "candle5m"
/// - "1h" or "1H" -> "candle1H"
/// - "1d" or "1D" -> "candle1D"
pub fn parse_timeframe_to_channel(timeframe: &str) -> Result<String> {
    let channel = match timeframe.to_lowercase().as_str() {
        "1m" => "candle1m",
        "5m" => "candle5m",
        "15m" => "candle15m",
        "30m" => "candle30m",
        "1h" => "candle1H",
        "4h" => "candle4H",
        "12h" => "candle12H",
        "1d" => "candle1D",
        "1w" => "candle1W",
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid timeframe: {}. Valid options: 1m, 5m, 15m, 30m, 1h, 4h, 12h, 1d, 1w",
                timeframe
            ))
        }
    };
    Ok(channel.to_string())
}

pub struct BitgetWsClient;

impl BitgetWsClient {
    pub async fn subscribe_tickers(
        inst_type: &str,
        inst_id: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<WsTickerData>>, Box<dyn std::error::Error>>
    {
        let url = "wss://ws.bitget.com/v2/ws/public";
        info!("Connecting to Bitget WebSocket: {}", url);

        let (ws_stream, _) = connect_async(url).await?;
        let (mut write, mut read) = ws_stream.split();

        let subscribe_msg = json!({
            "op": "subscribe",
            "args": [{
                "instType": inst_type,
                "channel": "ticker",
                "instId": inst_id
            }]
        });

        write
            .send(Message::Text(subscribe_msg.to_string().into()))
            .await?;

        let stream = async_stream::try_stream! {
            let mut last_ping = std::time::Instant::now();
            let ping_interval = std::time::Duration::from_secs(25);

            loop {
                if last_ping.elapsed() >= ping_interval {
                    if let Err(e) = write.send(Message::Text("ping".to_string().into())).await {
                        error!("Failed to send ping: {}", e);
                        break;
                    }
                    last_ping = std::time::Instant::now();
                }

                let msg_result = tokio::time::timeout(std::time::Duration::from_secs(1), read.next()).await;

                match msg_result {
                    std::result::Result::Ok(Some(std::result::Result::Ok(Message::Text(text)))) => {
                        if text == "pong" {
                            continue;
                        }

                        if let std::result::Result::Ok(response) = serde_json::from_str::<WsTickerResponse>(&text) {
                            for ticker in response.data {
                                yield ticker;
                            }
                        }
                    }
                    std::result::Result::Ok(Some(std::result::Result::Ok(Message::Close(_)))) => break,
                    std::result::Result::Ok(Some(std::result::Result::Err(e))) => {
                        error!("WS error: {}", e);
                        break;
                    }
                    std::result::Result::Ok(None) => break,
                    std::result::Result::Err(_) => continue, // Timeout, check ping
                    _ => continue,
                }
            }
        };

        std::result::Result::Ok(Box::pin(stream))
    }

    pub async fn subscribe_candlesticks(
        inst_type: &str,
        inst_id: &str,
        timeframe: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<WsCandleData>>, Box<dyn std::error::Error>>
    {
        let url = "wss://ws.bitget.com/v2/ws/public";
        info!("Connecting to Bitget WebSocket for candlesticks: {}", url);

        let channel = parse_timeframe_to_channel(timeframe).map_err(|e| format!("{}", e))?;

        let (ws_stream, _) = connect_async(url).await?;
        let (mut write, mut read) = ws_stream.split();

        let subscribe_msg = json!({
            "op": "subscribe",
            "args": [{
                "instType": inst_type,
                "channel": channel,
                "instId": inst_id
            }]
        });

        info!("Subscribing to candlesticks: {}", subscribe_msg);
        write
            .send(Message::Text(subscribe_msg.to_string().into()))
            .await?;

        let stream = async_stream::try_stream! {
            let mut last_ping = std::time::Instant::now();
            let ping_interval = std::time::Duration::from_secs(25);

            loop {
                if last_ping.elapsed() >= ping_interval {
                    if let Err(e) = write.send(Message::Text("ping".to_string().into())).await {
                        error!("Failed to send ping: {}", e);
                        break;
                    }
                    last_ping = std::time::Instant::now();
                }

                let msg_result = tokio::time::timeout(std::time::Duration::from_secs(1), read.next()).await;

                match msg_result {
                    std::result::Result::Ok(Some(std::result::Result::Ok(Message::Text(text)))) => {
                        if text == "pong" {
                            continue;
                        }

                        if let std::result::Result::Ok(response) = serde_json::from_str::<WsCandleResponse>(&text) {
                            for candle in response.data {
                                yield candle;
                            }
                        }
                    }
                    std::result::Result::Ok(Some(std::result::Result::Ok(Message::Close(_)))) => break,
                    std::result::Result::Ok(Some(std::result::Result::Err(e))) => {
                        error!("WS error: {}", e);
                        break;
                    }
                    std::result::Result::Ok(None) => break,
                    std::result::Result::Err(_) => continue, // Timeout, check ping
                    _ => continue,
                }
            }
        };

        std::result::Result::Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_multiple_prices() {
        let json = r#"{
            "code": "00000",
            "msg": "success",
            "requestTime": 1760676640447,
            "data": [
                {
                    "symbol": "BTCUSDT",
                    "price": "108895.8",
                    "indexPrice": "108964.6275376986964441",
                    "markPrice": "108896.2",
                    "ts": "1760676640448"
                },
                {
                    "symbol": "ETHUSDT",
                    "price": "2500.5",
                    "indexPrice": "2501.25",
                    "markPrice": "2500.75",
                    "ts": "1760676640448"
                }
            ]
        }"#;

        let all_prices = parse_price_response(json).unwrap();
        assert_eq!(all_prices.len(), 2);
        // assert_eq!(all_prices[0].price, 108895.8);
        // assert_eq!(all_prices[1].price, 2500.5);
    }

    #[test]
    fn test_parse_ws_ticker_response() {
        let json = r#"{
            "action": "snapshot",
            "arg": {
                "instType": "USDT-FUTURES",
                "channel": "ticker",
                "instId": "BTCUSDT"
            },
            "data": [
                {
                    "instId": "BTCUSDT",
                    "lastPr": "100000.5",
                    "bidPr": "100000.4",
                    "askPr": "100000.6",
                    "bidSz": "1.2",
                    "askSz": "0.5",
                    "high24h": "105000",
                    "low24h": "95000",
                    "baseVolume": "1.2",
                    "quoteVolume": "120000",
                    "openUtc": "100000",
                    "symbolType": "perpetual",
                    "symbol": "BTCUSDT",
                    "ts": "1620000000000"
                }
            ]
        }"#;

        let response: WsTickerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.action, "snapshot");
        assert_eq!(response.arg.inst_id, "BTCUSDT");
        assert_eq!(response.data[0].last_pr, "100000.5");
        assert_eq!(response.data[0].ts, "1620000000000");
    }

    #[test]
    fn test_parse_ws_candle_response() {
        let json = r#"{
            "action": "snapshot",
            "arg": {
                "instType": "USDT-FUTURES",
                "channel": "candle1m",
                "instId": "BTCUSDT"
            },
            "data": [
                {
                    "ts": "1609459200000",
                    "o": "29000.5",
                    "h": "29500.0",
                    "l": "28900.0",
                    "c": "29300.5",
                    "baseVol": "100.5",
                    "quoteVol": "2950000.0"
                }
            ]
        }"#;

        let response: WsCandleResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.action, "snapshot");
        assert_eq!(response.arg.channel, "candle1m");
        assert_eq!(response.arg.inst_id, "BTCUSDT");
        assert_eq!(response.data[0].timestamp, "1609459200000");
        assert_eq!(response.data[0].open, "29000.5");
        assert_eq!(response.data[0].high, "29500.0");
        assert_eq!(response.data[0].low, "28900.0");
        assert_eq!(response.data[0].close, "29300.5");
    }

    #[test]
    fn test_parse_timeframe_to_channel() {
        assert_eq!(parse_timeframe_to_channel("1m").unwrap(), "candle1m");
        assert_eq!(parse_timeframe_to_channel("5m").unwrap(), "candle5m");
        assert_eq!(parse_timeframe_to_channel("15m").unwrap(), "candle15m");
        assert_eq!(parse_timeframe_to_channel("30m").unwrap(), "candle30m");
        assert_eq!(parse_timeframe_to_channel("1h").unwrap(), "candle1H");
        assert_eq!(parse_timeframe_to_channel("1H").unwrap(), "candle1H");
        assert_eq!(parse_timeframe_to_channel("4h").unwrap(), "candle4H");
        assert_eq!(parse_timeframe_to_channel("12h").unwrap(), "candle12H");
        assert_eq!(parse_timeframe_to_channel("1d").unwrap(), "candle1D");
        assert_eq!(parse_timeframe_to_channel("1D").unwrap(), "candle1D");
        assert_eq!(parse_timeframe_to_channel("1w").unwrap(), "candle1W");
        assert_eq!(parse_timeframe_to_channel("1W").unwrap(), "candle1W");

        // Test invalid timeframe
        assert!(parse_timeframe_to_channel("invalid").is_err());
    }
}
