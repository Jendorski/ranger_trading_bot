use anyhow::Ok;
use anyhow::Result;
use anyhow::anyhow;
use std::env;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    /// API key / secret pair for your broker
    pub api_key: String,
    pub api_secret: String,

    /// Trading symbol (e.g. BTCUSDT)
    pub symbol: String,

    /// Polling interval in seconds
    #[serde(default = "default_interval")]
    pub poll_interval_secs: u64,

    pub redis_url: String,

    pub margin: f64,

    pub leverage: f64,

    pub risk_pct: f64,

    pub ranger_risk_pct: f64,

    pub scalp_price_difference: f64,
    pub ranger_price_difference: f64,

    pub profit_factor: f64,
}

fn default_interval() -> u64 {
    5
}

impl Config {
    /// Load from environment variables (dotenv recommended)
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        let api_key = env::var("API_KEY").map_err(|_| anyhow!("Missing API_KEY"))?;

        let api_secret = env::var("API_SECRET").map_err(|_| anyhow!("Missing API_SECRET"))?;

        let symbol = env::var("SYMBOL").unwrap_or_else(|_| "BTCUSDT".into());

        let poll_interval_secs: u64 = env::var("POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5);

        let redis_url = env::var("REDIS_URL").map_err(|_| anyhow!("Missing REDIS_URL"))?;

        let margin: f64 = env::var("MARGIN")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(50.00);

        let leverage = env::var("LEVERAGE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(20.00);

        let risk_pct = env::var("RISK_PERCENTAGE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.05); //5%

        let scalp_price_difference = env::var("SCALP_PRICE_DIFFERENCE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(400.0);

        let ranger_price_difference = env::var("RANGER_PRICE_DIFFERENCE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(1000.0);

        let profit_factor = env::var("PARTIAL_PROFIT_FACTOR")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(400.0);

        let ranger_risk_pct = env::var("RANGER_RISK_PERCENTAGE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.15); //15%

        Ok(Config {
            api_key,
            api_secret,
            symbol,
            poll_interval_secs,
            redis_url,
            margin,
            leverage,
            risk_pct,
            ranger_risk_pct,
            scalp_price_difference,
            ranger_price_difference,
            profit_factor,
        })
    }
}
