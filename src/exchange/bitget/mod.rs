use anyhow::{Ok, Result};
use async_trait::async_trait;
use log::info;
use serde::{Deserialize, Serialize};

//For binance: https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=100

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse {
    pub code: String,
    pub msg: String,
    #[serde(rename = "requestTime")]
    pub request_time: i64,
    pub data: Vec<Candle>,
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
    /// Return the latest candles
    async fn get_bitget_candles(&self, interval: String) -> Result<Vec<Candle>>;
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
    async fn get_bitget_candles(&self, interval: String) -> Result<Vec<Candle>> {
        let url = format!(
            "https://api.bitget.com/api/v2/mix/market/candles?symbol={}&granularity={}&limit=100&productType=usdt-futures",
            self.symbol, interval
        );
        info!("url: {:?}", url);
        let bitget = self.client.get(url).send().await?;

        let bit_text = bitget.text().await?;

        let response: ApiResponse = serde_json::from_str(&bit_text).unwrap();
        assert_eq!(response.code, "00000");
        assert_eq!(response.msg, "success");
        assert_eq!(response.data.len(), 100);

        let candles = response.data;

        Ok(candles)
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
