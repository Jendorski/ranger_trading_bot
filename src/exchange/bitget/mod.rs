use std::collections::HashMap;

use anyhow::{Ok, Result};
use async_trait::async_trait;
use chrono::Utc;
use log::info;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::{
    bot::{OpenPosition, Position},
    config::Config,
};

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
fn deserialize_string_to_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse::<i64>().map_err(serde::de::Error::custom)
}

fn deserialize_string_to_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
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
}

//#[async_trait]
pub trait FuturesCall {
    fn new() -> Self;

    /// Return the latest candles
    async fn new_futures_call(&self, open_position: OpenPosition) -> Result<PlaceOrderData>;

    fn return_bitget_headers(&self) -> Result<HeaderMap>;

    fn return_bitget_request_body(
        &self,
        open_position: OpenPosition,
    ) -> Result<HashMap<std::string::String, std::string::String>>;

    async fn modify_futures_order(&self, open_position: OpenPosition) -> Result<PlaceOrderData>;
}

/// Simple HTTP‑based mock of the `Exchange` trait – replace with your real SDK.
///
/// In this example we hit a public ticker endpoint (e.g. Binance).
pub struct HttpCandleData {
    pub client: reqwest::Client,
    pub(crate) symbol: String,
}

#[async_trait::async_trait]
impl CandleData for HttpCandleData {
    fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            symbol: String::from("BTCUSDT"),
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
}

//#[async_trait::async_trait]
impl FuturesCall for HttpCandleData {
    fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            symbol: String::new(),
        }
    }

    fn return_bitget_request_body(
        &self,
        open_position: OpenPosition,
    ) -> Result<HashMap<std::string::String, std::string::String>> {
        let mut side: &str = "buy";

        let pos_size = open_position.position_size.to_string();

        let entry_price = open_position.entry_price.to_string();

        let client_order_id = open_position.id.to_string();

        // let preset_stop_loss_price = open_position.sl.unwrap().to_string();

        // let preset_take_profit_price = open_position.tp.unwrap().to_string();

        if open_position.pos == Position::Long {
            side = "buy";
        }

        if open_position.pos == Position::Short {
            side = "sell";
        }

        let mut req_body: HashMap<String, String> = HashMap::<String, String>::new();

        req_body.insert(String::from("symbol"), String::from("BTCUSDT"));
        req_body.insert(String::from("side"), String::from(side));
        req_body.insert(String::from("orderType"), String::from("limit"));
        req_body.insert(String::from("size"), String::from(&pos_size));
        req_body.insert(String::from("price"), String::from(&entry_price));
        req_body.insert(String::from("timeInForce"), String::from("goodTillCancel"));
        req_body.insert(String::from("marginMode"), String::from("isolated"));
        req_body.insert(String::from("productType"), String::from("USDT-FUTURES"));
        req_body.insert(String::from("marginCoin"), String::from("USDT"));
        req_body.insert(String::from("clientOid"), String::from(&client_order_id));

        Ok(req_body)
    }

    async fn modify_futures_order(&self, open_position: OpenPosition) -> Result<PlaceOrderData> {
        let headers = self.return_bitget_headers()?;

        let mut req_body = self.return_bitget_request_body(open_position)?;

        let preset_stop_loss_price = open_position.sl.unwrap().to_string();

        let preset_take_profit_price = open_position.tp.unwrap().to_string();

        let new_size = open_position.position_size.to_string();

        req_body.remove("size");

        req_body.insert(String::from("newSize"), String::from(&new_size));
        req_body.insert(
            String::from("newPresetStopSurplusPrice"),
            String::from(&preset_take_profit_price),
        );
        req_body.insert(
            String::from("newPresetStopLossPrice"),
            String::from(&preset_stop_loss_price),
        );

        let url = format!("https://api.bitget.com/api/v2/mix/order/modify-order");
        info!("url: {:?}", url);

        let bitget = self
            .client
            .post(url)
            .headers(headers)
            .json(&req_body)
            .send()
            .await?;

        let bit_text = bitget.text().await?;
        info!("bit_text::modify_futures_order -> {:?}", bit_text);

        let response: ApiResponse<PlaceOrderData> =
            serde_json::from_str(&bit_text).unwrap_or(ApiResponse {
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

    fn return_bitget_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();

        let config = Config::from_env()?;

        headers.insert("ACCESS-KEY", config.api_key.parse().unwrap());
        headers.insert("ACCESS-SIGN", config.api_secret.parse().unwrap());
        headers.insert("ACCESS-PASSPHRASE", config.passphrase.parse().unwrap());
        headers.insert(
            "ACCESS-TIMESTAMP",
            Utc::now().timestamp_millis().to_string().parse().unwrap(),
        );
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("locale", "english".parse().unwrap());

        Ok(headers)
    }

    async fn new_futures_call(&self, open_position: OpenPosition) -> Result<PlaceOrderData> {
        let url = format!("https://api.bitget.com/api/v2/mix/order/place-order");
        info!("url: {:?}", url);

        let preset_stop_surplus_price = open_position.tp.unwrap().to_string();
        let preset_stop_loss_price = open_position.sl.unwrap().to_string();

        let mut req_body = self.return_bitget_request_body(open_position)?;

        req_body.insert(
            String::from("presetStopSurplusPrice"),
            String::from(&preset_stop_surplus_price),
        );
        req_body.insert(
            String::from("presetStopLossPrice"),
            String::from(&preset_stop_loss_price),
        );

        let headers = self.return_bitget_headers()?;
        let bitget = self
            .client
            .post(url)
            .form(&req_body)
            .headers(headers)
            .send()
            .await?;

        let bit_text = bitget.text().await?;
        info!("bit_text::new_futures_call -> {:?}", bit_text);

        let response: ApiResponse<PlaceOrderData> =
            serde_json::from_str(&bit_text).unwrap_or(ApiResponse {
                code: "4000".to_string(),
                msg: "Error".to_string(),
                request_time: Utc::now().timestamp(),
                data: PlaceOrderData {
                    client_oid: String::from("Failed to place order"),
                    order_id: String::from("Failed to place order"),
                },
            });
        info!("response::new_futures_call -> {:?}", response);

        if response.code != "00000" {
            return Ok(PlaceOrderData {
                client_oid: String::from("Failed to place order"),
                order_id: String::from("Failed to place order"),
            });
        }

        let order = response.data;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_price_response() {
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
                }
            ]
        }"#;

        let prices = get_prices(json).unwrap();
        // assert_eq!(prices.price, 108895.8);
        // assert_eq!(prices.index_price, 108964.6275376986964441);
        assert_eq!(prices.mark_price, 108896.2);
    }

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
}
