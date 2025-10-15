use std::sync::Arc;
use std::{error::Error, time::Duration};

use log::{info, warn};
use tokio::time;

use crate::cache::RedisClient;
use crate::config::Config;
use crate::exchange::{Exchange, OrderSide};
use crate::graph::Graph;

mod bot;
mod cache;
mod config;
mod exchange;
mod graph;

// use btc_trading_bot::{
//     bot::Bot,
//     config::Config,
//     exchange::{Exchange, OrderSide},
// };

/// Simple HTTP‑based mock of the `Exchange` trait – replace with your real SDK.
///
/// In this example we hit a public ticker endpoint (e.g. Binance).
struct HttpExchange {
    client: reqwest::Client,
    symbol: String,
}

#[async_trait::async_trait]
impl Exchange for HttpExchange {
    async fn get_current_price(&self) -> Result<f64, anyhow::Error> {
        // Example: Binance spot ticker

        //Bitget Futures Price API: https://api.bitget.com/api/v2/mix/market/symbol-price?productType=usdt-futures&symbol=BTCUSDT
        let resp = self
            .client
            .get(format!(
                "https://api.binance.com/api/v3/ticker/price?symbol={}",
                self.symbol
            ))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;
        let price: f64 = resp["price"].as_str().unwrap_or("0").parse()?;
        Ok(price)
    }

    async fn place_market_order(&self, side: OrderSide, amount: f64) -> Result<f64, anyhow::Error> {
        // For demo purposes we just log and pretend the order filled at current price.
        let price = self.get_current_price().await?;
        info!(
            "Mock market {:?} for {:.6} {} at {price:.2}",
            side, amount, self.symbol
        );
        Ok(price)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv::dotenv().ok();

    // Logging
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    // 1️⃣ Load config
    let mut cfg = Config::from_env()?;
    info!("Loaded config: {:?}", cfg);

    let mut binding = RedisClient::connect(&cfg.redis_url).await?;
    let redis_conn = binding.get_conn();

    // 2️⃣ Create exchange instance (replace with real SDK in production)
    let exchange = Arc::new(HttpExchange {
        client: reqwest::Client::new(),
        symbol: cfg.symbol.clone(),
    });

    // 3️⃣ Bot state
    let mut bot = bot::Bot::new(redis_conn.clone()).await?;

    // 4️⃣ Poll loop
    let mut interval = time::interval(Duration::from_secs(cfg.poll_interval_secs));

    loop {
        interval.tick().await;

        match exchange.get_current_price().await {
            Ok(price) => {
                if let Err(e) = bot.run_cycle(price, exchange.as_ref(), &mut cfg).await {
                    eprintln!("Error during cycle: {e}");
                }
            }
            Err(err) => eprintln!("Failed to fetch price: {err}"),
        }

        if Graph::is_midnight() {
            warn!("It's midnight now!");
            Graph::prepare_cumulative_weekly_monthly(redis_conn.clone()).await?;

            Graph::all_trade_compute(redis_conn.clone()).await?;
        }
    }
}
