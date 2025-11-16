use anyhow::{Ok, Result};
use serde::{Deserialize, Serialize};

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
            price: item.price.parse().unwrap_or(0.0),
            index_price: item.index_price.parse().unwrap_or(0.0),
            mark_price: item.mark_price.parse().unwrap_or(0.0),
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
