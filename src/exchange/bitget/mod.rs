use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;

use anyhow::{Ok, Result};
use async_trait::async_trait;
use chrono::Utc;
use log::info;
use reqwest::{Client, header::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    bot::{OpenPosition, Position},
    config::Config,
    encryption,
    helper::Helper,
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

    fn return_bitget_headers(&self, method: &str, url: &str, body: &str) -> Result<HeaderMap>;

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
        let api_key = &self.config.api_key;
        let secret = &self.config.api_secret;
        let passphrase = &self.config.passphrase;

        let base_url = "https://api.bitget.com";
        let path = "/api/v2/mix/order/modify-order";
        let method = "POST";

        let preset_stop_surplus_price = open_position.tp.unwrap().to_string();
        let preset_stop_loss_price = open_position.sl.unwrap().to_string();

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
            "presetStopSurplusPrice": preset_stop_surplus_price,
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

    fn return_bitget_headers(&self, method: &str, url: &str, body: &str) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();

        let config = Config::from_env()?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let concat_string = format!("{}{}{}{}", timestamp, method, url, body);
        info!("concat_string: {:?}", concat_string);

        let key = config.api_secret.as_bytes();
        // Create the HMAC-SHA256 hasher with the key
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC can take key of any size");

        // Input the message
        mac.update(timestamp.as_bytes());

        // 3. Finalize the HMAC calculation to get the raw signature bytes
        let result = mac.finalize();

        let signature_bytes = result.into_bytes();

        // 4. Base64 encode the resulting bytes into a printable string
        let base64_encoded_signature = base64::encode(&signature_bytes);

        headers.insert("ACCESS-KEY", config.api_key.parse().unwrap());
        headers.insert("ACCESS-SIGN", base64_encoded_signature.parse().unwrap());
        headers.insert("ACCESS-PASSPHRASE", config.passphrase.parse().unwrap());
        headers.insert(
            "ACCESS-TIMESTAMP",
            Utc::now().timestamp_millis().to_string().parse().unwrap(),
        );
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("locale", "english".parse().unwrap());
        info!("headers: {:?}", headers);

        Ok(headers)
    }

    async fn new_futures_call(&self, open_position: OpenPosition) -> Result<PlaceOrderData> {
        let api_key = &self.config.api_key;
        let secret = &self.config.api_secret;
        let passphrase = &self.config.passphrase;

        let base_url = "https://api.bitget.com";
        let path = "/api/v2/mix/order/place-order";
        let method = "POST";

        //let preset_stop_surplus_price = open_position.tp.unwrap().to_string();
        let preset_stop_loss_price = Helper::truncate_to_1_dp(open_position.sl.unwrap_or(0.00));

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
}
