use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::bot::{OpenPosition, Position};
use crate::exchange::bitget::{deserialize_string_to_f64, ApiResponse};

#[derive(Debug, Clone, Copy)]
pub enum ExecutionType {
    Maker,
    Taker,
}

#[derive(Debug, Clone, Copy)]
pub enum ExitReason {
    TakeProfit,
    StopLoss,
}

#[derive(Debug, Clone)]
pub struct BitgetFuturesFees {
    pub maker_fee: f64, // 0.02%
    pub taker_fee: f64, // 0.06%
    pub funding_rate: f64,
    pub redis_conn: redis::aio::MultiplexedConnection,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VipFeeRate {
    pub level: String,
    pub deal_amount: String,
    pub asset_amount: String,
    #[serde(deserialize_with = "deserialize_string_to_f64")]
    pub taker_fee_rate: f64,
    #[serde(deserialize_with = "deserialize_string_to_f64")]
    pub maker_fee_rate: f64,
    pub btc_withdraw_amount: String,
    pub usdt_withdraw_amount: String,
}

impl BitgetFuturesFees {
    pub fn new(conn: redis::aio::MultiplexedConnection) -> Self {
        Self {
            maker_fee: 0.0,
            taker_fee: 0.0,
            funding_rate: 0.0,
            redis_conn: conn,
        }
    }

    pub fn from_vip_data(conn: redis::aio::MultiplexedConnection, vip_data: &VipFeeRate) -> Self {
        Self {
            maker_fee: vip_data.maker_fee_rate,
            taker_fee: vip_data.taker_fee_rate,
            funding_rate: 0.0,
            redis_conn: conn,
        }
    }

    pub fn for_level(self, level: &str, rates: &[VipFeeRate]) -> Option<Self> {
        rates
            .iter()
            .find(|r| r.level == level)
            .map(|r| Self::from_vip_data(self.redis_conn, r))
    }

    pub async fn fee_on_notional(&self, price: f64, size: f64, exec: ExecutionType) -> f64 {
        let vip_fee_rates_resp = Self::get_vip_fee_rates(self).await;

        let vip_fee_rates = vip_fee_rates_resp.unwrap_or(Vec::from([VipFeeRate {
            level: "0".to_string(),
            deal_amount: "0".to_string(),
            asset_amount: "0".to_string(),
            taker_fee_rate: 0.0,
            maker_fee_rate: 0.0,
            btc_withdraw_amount: "0".to_string(),
            usdt_withdraw_amount: "0".to_string(),
        }]));

        let fees = vip_fee_rates.first().unwrap();

        let maker_fee = fees.maker_fee_rate;
        let taker_fee = fees.taker_fee_rate;

        let notional = price * size;
        let rate = match exec {
            ExecutionType::Maker => maker_fee,
            ExecutionType::Taker => taker_fee,
        };
        notional * rate
    }

    pub async fn pnl_for_exit(side: Position, entry_price: f64, exit_price: f64, size: f64) -> f64 {
        match side {
            Position::Long => (exit_price - entry_price) * size,
            Position::Short => (entry_price - exit_price) * size,
            Position::Flat => 0.00,
        }
    }

    //Always using taker fee for entry
    pub async fn calc_margin_for_entry(
        &self,
        entry_price: f64,
        position_size: f64,
        margin: f64,
    ) -> f64 {
        let entry_fee = self
            .fee_on_notional(entry_price, position_size, ExecutionType::Taker)
            .await;
        margin - entry_fee
    }

    pub async fn calc_pnl_for_exit(
        &self,
        open_position: OpenPosition,
        current_price: f64,
    ) -> (f64, f64) {
        let exit_fee = self
            .fee_on_notional(
                current_price,
                open_position.position_size,
                ExecutionType::Taker,
            )
            .await;
        let pnl = Self::pnl_for_exit(
            open_position.pos,
            open_position.entry_price,
            current_price,
            open_position.position_size,
        )
        .await;
        (pnl - exit_fee, exit_fee)
    }

    pub async fn get_vip_fee_rates(&self) -> Result<Vec<VipFeeRate>, anyhow::Error> {
        let key = "bitget::vip_fee_rates";
        let mut conn = self.redis_conn.clone();

        // Try to get from Redis
        let cached: Option<String> = conn.get(key).await.unwrap_or(None);
        if let Some(cached_json) = cached {
            if let Ok(rates) = serde_json::from_str::<Vec<VipFeeRate>>(&cached_json) {
                return Ok(rates);
            }
        }

        let url = "https://api.bitget.com/api/v2/mix/market/vip-fee-rate";

        let response = reqwest::blocking::get(url)?;

        let text = response.text()?;
        let api_response: ApiResponse<Vec<VipFeeRate>> = serde_json::from_str(&text)?;

        if api_response.code != "00000" {
            return Err(anyhow::anyhow!("Bitget API error: {}", api_response.msg));
        }

        // Cache the response
        if let Ok(json) = serde_json::to_string(&api_response.data) {
            let _: () = conn.set_ex(key, json, 86400).await?; // 24 hours
        }

        Ok(api_response.data)
    }
}
