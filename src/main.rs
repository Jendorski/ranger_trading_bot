use std::sync::Arc;
use std::{error::Error, time::Duration};

use log::{info, warn};
use redis::aio::MultiplexedConnection;
use tokio::time;

use crate::cache::RedisClient;
use crate::config::Config;
use crate::exchange::{Exchange, HttpExchange};
use crate::graph::Graph;
use crate::helper::Helper;

mod bot;
mod cache;
mod config;
mod encryption;
mod exchange;
mod graph;
mod helper;
mod trackers;

async fn run_bot(
    redis_conn: &mut MultiplexedConnection,
    bot: &mut bot::Bot<'_>,
    exchange: Arc<HttpExchange>,
    cfg: Config,
) -> Result<(), Box<dyn Error>> {
    let mut graph = graph::Graph::new();

    // 4️⃣ Poll loop
    let mut interval = time::interval(Duration::from_secs(cfg.poll_interval_secs));

    loop {
        interval.tick().await;

        match exchange.get_current_price().await {
            Ok(price) => {
                info!("Price = {:.2}", price,);
                if let Err(e) = //bot.test().await
                    bot.run_cycle(price, exchange.as_ref()).await
                {
                    eprintln!("Error during cycle: {e}");
                }
            }
            Err(err) => eprintln!("Failed to fetch price: {err}"),
        }

        if Helper::is_midnight() {
            warn!("It's midnight now!");
            Graph::prepare_cumulative_weekly_monthly(&mut graph, redis_conn.clone()).await?;
        }
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
    let cfg = Config::from_env()?;
    let config_clone = cfg.clone();

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

    let mut bot_conn = redis_conn.clone();
    info!("Starting bot loop...");

    if let Err(e) = //bot.test().await {
        run_bot(&mut bot_conn, &mut bot, exchange.clone(), config_clone).await
    {
        log::error!("Bot loop error: {}", e);
    }

    Ok(())
}
