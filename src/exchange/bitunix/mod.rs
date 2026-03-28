#![allow(dead_code)]
#![allow(clippy::uninlined_format_args)]
use anyhow::Result;
use chrono::Utc;
use log::info;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::bot::{OpenPosition, Position};
use crate::config::Config;
use crate::exchange::bitget::PlaceOrderData;
use crate::helper::Helper;

pub mod fees;
pub mod ws;

const BASE_URL: &str = "https://fapi.bitunix.com";

// ─── Signing ────────────────────────────────────────────────────────────────

/// Generate a 32-char nonce (UUID v4 hex without dashes).
pub fn generate_nonce() -> String {
    Uuid::new_v4().to_string().replace('-', "")
}

/// Sort query params ascending by key and join as "key1val1key2val2".
pub fn build_sorted_params(params: &[(&str, &str)]) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by_key(|(k, _)| *k);
    sorted.iter().map(|(k, v)| format!("{}{}", k, v)).collect()
}

/// Two-stage plain SHA256 signing required by Bitunix.
///
/// digest = SHA256( nonce + timestamp + api_key + sorted_query_params + body )
/// sign   = SHA256( digest_hex + secret_key )
pub fn bitunix_sign(
    nonce: &str,
    timestamp: &str,
    api_key: &str,
    sorted_params: &str,
    body_str: &str,
    secret_key: &str,
) -> String {
    let first_input = format!("{}{}{}{}{}", nonce, timestamp, api_key, sorted_params, body_str);
    let mut h1 = Sha256::new();
    h1.update(first_input.as_bytes());
    let digest_hex = format!("{:x}", h1.finalize());

    let second_input = format!("{}{}", digest_hex, secret_key);
    let mut h2 = Sha256::new();
    h2.update(second_input.as_bytes());
    format!("{:x}", h2.finalize())
}

fn make_auth_headers(
    api_key: &str,
    secret: &str,
    sorted_params: &str,
    body_str: &str,
) -> reqwest::header::HeaderMap {
    let nonce = generate_nonce();
    let timestamp = Utc::now().timestamp_millis().to_string();
    let sign = bitunix_sign(&nonce, &timestamp, api_key, sorted_params, body_str, secret);

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("api-key", api_key.parse().unwrap());
    headers.insert("nonce", nonce.parse().unwrap());
    headers.insert("timestamp", timestamp.parse().unwrap());
    headers.insert("sign", sign.parse().unwrap());
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers
}

// ─── API response wrapper ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct BitunixApiResponse<T> {
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

// ─── Market data structs ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TickerData {
    pub symbol: String,
    #[serde(rename = "lastPrice")]
    pub last_price: String,
    #[serde(rename = "markPrice")]
    pub mark_price: String,
}

#[derive(Debug, Deserialize)]
pub struct KlineData {
    pub o: String,
    pub h: String,
    pub l: String,
    pub c: String,
    pub b: String,
    pub q: String,
    pub time: i64,
}

#[derive(Debug, Deserialize)]
pub struct FundingRateData {
    pub symbol: String,
    #[serde(rename = "fundingRate")]
    pub funding_rate: String,
    #[serde(rename = "nextFundingTime")]
    pub next_funding_time: i64,
    #[serde(rename = "markPrice")]
    pub mark_price: String,
}

// ─── Trade structs ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PlaceOrderResponse {
    #[serde(rename = "orderId")]
    pub order_id: String,
    #[serde(rename = "clientId")]
    pub client_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TpslOrderResponse {
    #[serde(rename = "orderId")]
    pub order_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PendingPosition {
    #[serde(rename = "positionId")]
    pub position_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: String,
}

// ─── HTTP client ─────────────────────────────────────────────────────────────

pub struct BitunixHttpClient {
    pub client: reqwest::Client,
    pub symbol: String,
    pub api_key: String,
    pub api_secret: String,
    pub maker_fee: f64,
    pub taker_fee: f64,
}

impl BitunixHttpClient {
    pub fn new(config: &Config) -> Self {
        Self {
            client: reqwest::Client::new(),
            symbol: config.symbol.clone(),
            api_key: config.bitunix_api_key.clone(),
            api_secret: config.bitunix_api_secret.clone(),
            maker_fee: config.bitunix_maker_fee,
            taker_fee: config.bitunix_taker_fee,
        }
    }

    // ── Public endpoints ──────────────────────────────────────────────────

    pub async fn get_current_price(&self) -> Result<f64> {
        let url = format!(
            "{}/api/v1/futures/market/get_tickers?symbol={}",
            BASE_URL, self.symbol
        );
        let resp = self.client.get(&url).send().await?.text().await?;
        let parsed: BitunixApiResponse<Vec<TickerData>> = serde_json::from_str(&resp)
            .map_err(|e| anyhow::anyhow!("parse ticker: {e}, body: {resp}"))?;

        if parsed.code != 0 {
            return Err(anyhow::anyhow!("Bitunix ticker error: {}", parsed.msg));
        }
        let data = parsed
            .data
            .and_then(|v| v.into_iter().next())
            .ok_or_else(|| anyhow::anyhow!("No ticker data"))?;

        Ok(data.mark_price.parse()?)
    }

    pub async fn get_funding_rate(&self) -> Result<f64> {
        let url = format!(
            "{}/api/v1/futures/market/funding_rate?symbol={}",
            BASE_URL, self.symbol
        );
        let resp = self.client.get(&url).send().await?.text().await?;
        let parsed: BitunixApiResponse<Vec<FundingRateData>> = serde_json::from_str(&resp)
            .map_err(|e| anyhow::anyhow!("parse funding rate: {e}, body: {resp}"))?;

        if parsed.code != 0 {
            return Err(anyhow::anyhow!("Bitunix funding rate error: {}", parsed.msg));
        }
        let data = parsed
            .data
            .and_then(|v| v.into_iter().next())
            .ok_or_else(|| anyhow::anyhow!("No funding rate data"))?;

        Ok(data.funding_rate.parse().unwrap_or(0.0))
    }

    // ── Authenticated endpoints ───────────────────────────────────────────

    /// Fetch the current open position ID for this symbol.
    /// Called immediately after place_order to retrieve Bitunix's positionId.
    pub async fn get_pending_position_id(&self) -> Result<Option<String>> {
        let params = [("symbol", self.symbol.as_str())];
        let sorted = build_sorted_params(&params);
        let url = format!(
            "{}/api/v1/futures/position/get_pending_positions?symbol={}",
            BASE_URL, self.symbol
        );
        let headers = make_auth_headers(&self.api_key, &self.api_secret, &sorted, "");
        let resp = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await?
            .text()
            .await?;

        info!("bitunix get_pending_positions: {resp}");
        let parsed: BitunixApiResponse<Vec<PendingPosition>> = serde_json::from_str(&resp)
            .map_err(|e| anyhow::anyhow!("parse positions: {e}, body: {resp}"))?;

        if parsed.code != 0 {
            return Ok(None);
        }
        Ok(parsed
            .data
            .and_then(|v| v.into_iter().next().map(|p| p.position_id)))
    }

    /// Place a new market entry order. SL is embedded in the order body.
    pub async fn place_order(&self, open_position: &OpenPosition) -> Result<PlaceOrderData> {
        let (side, trade_side) = match open_position.pos {
            Position::Long => ("BUY", "OPEN"),
            Position::Short => ("SELL", "OPEN"),
            Position::Flat => return Err(anyhow::anyhow!("Cannot place order for Flat position")),
        };

        let sl_price = open_position
            .sl
            .map(|s| Helper::truncate_to_1_dp(Helper::decimal_to_f64(s)).to_string())
            .unwrap_or_default();

        let body_value = serde_json::json!({
            "symbol":      self.symbol,
            "qty":         open_position.position_size.to_string(),
            "side":        side,
            "tradeSide":   trade_side,
            "orderType":   "MARKET",
            "reduceOnly":  false,
            "clientId":    open_position.id.to_string(),
            "slPrice":     sl_price,
            "slStopType":  "MARK_PRICE",
            "slOrderType": "MARKET"
        });

        let body_str = serde_json::to_string(&body_value)?;
        let headers = make_auth_headers(&self.api_key, &self.api_secret, "", &body_str);

        let resp = self
            .client
            .post(format!("{}/api/v1/futures/trade/place_order", BASE_URL))
            .headers(headers)
            .body(body_str)
            .send()
            .await?
            .text()
            .await?;

        info!("bitunix place_order response: {resp}");

        let parsed: BitunixApiResponse<PlaceOrderResponse> = serde_json::from_str(&resp)
            .map_err(|e| anyhow::anyhow!("parse place_order: {e}, body: {resp}"))?;

        if parsed.code != 0 {
            return Ok(PlaceOrderData {
                client_oid: "Failed to place order".into(),
                order_id: "Failed to place order".into(),
            });
        }

        let data = parsed
            .data
            .ok_or_else(|| anyhow::anyhow!("No data in place_order response"))?;

        Ok(PlaceOrderData {
            client_oid: data.client_id.unwrap_or_default(),
            order_id: data.order_id,
        })
    }

    /// Close an entire position at market price by positionId.
    pub async fn flash_close_position(&self, position_id: &str) -> Result<PlaceOrderData> {
        let body_value = serde_json::json!({ "positionId": position_id });
        let body_str = serde_json::to_string(&body_value)?;
        let headers = make_auth_headers(&self.api_key, &self.api_secret, "", &body_str);

        let resp = self
            .client
            .post(format!(
                "{}/api/v1/futures/trade/flash_close_position",
                BASE_URL
            ))
            .headers(headers)
            .body(body_str)
            .send()
            .await?
            .text()
            .await?;

        info!("bitunix flash_close response: {resp}");

        Ok(PlaceOrderData {
            client_oid: position_id.to_string(),
            order_id: position_id.to_string(),
        })
    }

    /// Place a partial-close SELL/BUY CLOSE market order for a given qty.
    /// Used for partial TP ladder steps (Bitunix flash_close closes the whole position).
    pub async fn close_partial(
        &self,
        open_position: &OpenPosition,
        qty_to_close: &str,
    ) -> Result<PlaceOrderData> {
        let side = match open_position.pos {
            Position::Long => "SELL",
            Position::Short => "BUY",
            Position::Flat => return Err(anyhow::anyhow!("Cannot close Flat position")),
        };

        let body_value = serde_json::json!({
            "symbol":    self.symbol,
            "qty":       qty_to_close,
            "side":      side,
            "tradeSide": "CLOSE",
            "orderType": "MARKET",
            "reduceOnly": true,
        });

        let body_str = serde_json::to_string(&body_value)?;
        let headers = make_auth_headers(&self.api_key, &self.api_secret, "", &body_str);

        let resp = self
            .client
            .post(format!("{}/api/v1/futures/trade/place_order", BASE_URL))
            .headers(headers)
            .body(body_str)
            .send()
            .await?
            .text()
            .await?;

        info!("bitunix close_partial response: {resp}");

        let parsed: BitunixApiResponse<PlaceOrderResponse> = serde_json::from_str(&resp)
            .map_err(|e| anyhow::anyhow!("parse close_partial: {e}, body: {resp}"))?;

        if parsed.code != 0 {
            return Ok(PlaceOrderData {
                client_oid: "Failed to close partial".into(),
                order_id: "Failed to close partial".into(),
            });
        }

        let data = parsed
            .data
            .ok_or_else(|| anyhow::anyhow!("No data in close_partial response"))?;

        Ok(PlaceOrderData {
            client_oid: data.client_id.unwrap_or_default(),
            order_id: data.order_id,
        })
    }

    /// Register the initial TP/SL on a position (call once after opening).
    pub async fn place_position_tpsl(
        &self,
        position_id: &str,
        tp_price: Option<f64>,
        sl_price: Option<f64>,
    ) -> Result<String> {
        self.tpsl_request(
            "/api/v1/futures/tpsl/position/place_order",
            position_id,
            tp_price,
            sl_price,
        )
        .await
    }

    /// Update the TP/SL on an existing position (every subsequent ladder step).
    pub async fn modify_position_tpsl(
        &self,
        position_id: &str,
        tp_price: Option<f64>,
        sl_price: Option<f64>,
    ) -> Result<String> {
        self.tpsl_request(
            "/api/v1/futures/tpsl/position/modify_order",
            position_id,
            tp_price,
            sl_price,
        )
        .await
    }

    async fn tpsl_request(
        &self,
        path: &str,
        position_id: &str,
        tp_price: Option<f64>,
        sl_price: Option<f64>,
    ) -> Result<String> {
        let mut body_map = serde_json::Map::new();
        body_map.insert(
            "symbol".into(),
            serde_json::Value::String(self.symbol.clone()),
        );
        body_map.insert(
            "positionId".into(),
            serde_json::Value::String(position_id.to_string()),
        );

        if let Some(tp) = tp_price {
            body_map.insert(
                "tpPrice".into(),
                Helper::truncate_to_1_dp(tp).to_string().into(),
            );
            body_map.insert("tpStopType".into(), "MARK_PRICE".into());
            body_map.insert("tpOrderType".into(), "MARKET".into());
        }
        if let Some(sl) = sl_price {
            body_map.insert(
                "slPrice".into(),
                Helper::truncate_to_1_dp(sl).to_string().into(),
            );
            body_map.insert("slStopType".into(), "MARK_PRICE".into());
            body_map.insert("slOrderType".into(), "MARKET".into());
        }

        let body_str = serde_json::to_string(&serde_json::Value::Object(body_map))?;
        let headers = make_auth_headers(&self.api_key, &self.api_secret, "", &body_str);

        let resp = self
            .client
            .post(format!("{}{}", BASE_URL, path))
            .headers(headers)
            .body(body_str)
            .send()
            .await?
            .text()
            .await?;

        info!("bitunix tpsl {path} response: {resp}");

        let parsed: BitunixApiResponse<TpslOrderResponse> = serde_json::from_str(&resp)
            .map_err(|e| anyhow::anyhow!("parse tpsl: {e}, body: {resp}"))?;

        if parsed.code != 0 {
            return Err(anyhow::anyhow!("Bitunix TPSL error: {}", parsed.msg));
        }

        Ok(parsed.data.map(|d| d.order_id).unwrap_or_default())
    }
}
