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

    /// Size of each position in lots or units
    pub order_size: f64,

    /// Polling interval in seconds
    #[serde(default = "default_interval")]
    pub poll_interval_secs: u64,

    pub redis_url: String,
}

fn default_interval() -> u64 {
    15
}

impl Config {
    /// Load from environment variables (dotenv recommended)
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        let api_key = env::var("API_KEY").map_err(|_| anyhow!("Missing API_KEY"))?;

        let api_secret = env::var("API_SECRET").map_err(|_| anyhow!("Missing API_SECRET"))?;

        let symbol = env::var("SYMBOL").unwrap_or_else(|_| "BTCUSDT".into());

        let order_size: f64 = env::var("ORDER_SIZE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.001);

        let poll_interval_secs: u64 = env::var("POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(15);

        let redis_url = env::var("REDIS_URL").map_err(|_| anyhow!("Missing REDIS_URL"))?;

        Ok(Config {
            api_key,
            api_secret,
            symbol,
            order_size,
            poll_interval_secs,
            redis_url,
        })
    }
}
