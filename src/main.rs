use std::error::Error;
use std::sync::Arc;

use log::info;

use crate::cache::RedisClient;
use crate::config::Config;
use crate::exchange::HttpExchange;

mod bot;
mod cache;
mod config;
mod encryption;
mod exchange;
mod graph;
mod helper;
mod trackers;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv::dotenv().ok();

    // Logging
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    // 1️⃣ Load config
    let cfg = Config::from_env()?;

    let binding = RedisClient::connect(&cfg.redis_url).await?;
    let redis_conn = binding.get_multiplexed_connection();

    // 2️⃣ Create exchange instance (replace with real SDK in production)
    let exchange = Arc::new(HttpExchange {
        client: reqwest::Client::new(),
        symbol: cfg.symbol.clone(),
    });

    // 3️⃣ Bot state
    let mut bot = bot::Bot::new(redis_conn.clone(), &cfg).await?;
    // let mut scalper = bot::scalper::ScalperBot::new(redis_conn.clone()).await?;

    let smc_conn = redis_conn.clone();
    let smc_config = cfg.clone();
    let _smc_handle = tokio::spawn(async move {
        trackers::smart_money_concepts::smc_loop(smc_conn, smc_config).await;
    });

    // let _momentum_websocket_handler = tokio::spawn(async move {
    //     let _: () = momentum::start_live_tracking().await.unwrap();
    // });

    let _tracker_ichimoku = tokio::spawn(async move {
        if let Err(e) = trackers::ichimoku::ichimoku_loop().await {
            log::error!("Ichimoku tracker error: {}", e);
        }
    });

    info!("Starting bot loop...");

    if let Err(e) = bot.start_live_trading(exchange.as_ref()).await {
        log::error!("Bot loop error: {}", e);
    }

    Ok(())
}
