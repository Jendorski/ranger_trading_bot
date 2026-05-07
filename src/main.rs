use std::error::Error;
use std::sync::Arc;

use reqwest::Client;

use log::info;

use crate::cache::RedisClient;
use crate::config::{Config, ExchangeType};
use crate::exchange::HttpExchange;
use crate::exchange::BitunixExchange;

mod api;
mod bot;
mod cache;
mod calendar;
mod config;
mod data;
mod encryption;
mod exchange;
mod graph;
mod helper;
mod regime;
mod tasks;
mod trackers;
mod types;

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

    // 2️⃣ Seed historical candle data for all timeframes before anything else starts
    data::ensure_seeds_ready().await;

    // Single shared HTTP client — one connection pool for the entire process.
    let http = Arc::new(Client::new());

    // 3️⃣ Create exchange instance based on EXCHANGE env var
    let exchange: Arc<dyn crate::exchange::Exchange> = match cfg.exchange {
        ExchangeType::Bitunix => Arc::new(BitunixExchange::new(&cfg)),
        ExchangeType::Bitget => Arc::new(HttpExchange {
            client: (*http).clone(),
            symbol: cfg.symbol.clone(),
            redis_conn: redis_conn.clone(),
        }),
    };

    // 4️⃣ Bot state
    let mut bot = bot::Bot::new(redis_conn.clone(), &cfg).await?;

    let mut task_set = tasks::spawn_background_tasks(redis_conn.clone(), &cfg, Arc::clone(&http)).await;

    // Supervisor: watches every background task for unexpected exits or panics.
    // Dropping the JoinSet would abort all tasks, so it must live here for the
    // process lifetime — moving it into this task achieves that.
    tokio::spawn(async move {
        while let Some(result) = task_set.join_next().await {
            match result {
                Ok(()) => log::warn!("[supervisor] A background task returned — this should not happen"),
                Err(e) if e.is_panic() => log::error!("[supervisor] A background task panicked: {e:?}"),
                Err(e) => log::error!("[supervisor] A background task was cancelled: {e:?}"),
            }
        }
        log::error!("[supervisor] All background tasks have stopped");
    });

    info!("Starting bot loop...");

    let bot_result = match cfg.exchange {
        ExchangeType::Bitunix => bot.start_live_trading_bitunix(exchange.as_ref()).await,
        ExchangeType::Bitget => bot.start_live_trading(exchange.as_ref()).await,
    };
    if let Err(e) = bot_result {
        log::error!("Bot loop error: {e}");
    }

    Ok(())
}
