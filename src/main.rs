use std::sync::Arc;
use std::{error::Error, time::Duration};

use log::{info, warn};
use tokio::time;

use crate::cache::RedisClient;
use crate::config::Config;
use crate::exchange::{Exchange, HttpExchange};
use crate::graph::Graph;
use crate::helper::Helper;

mod bot;
mod cache;
mod config;
mod exchange;
mod graph;
mod helper;

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
    let mut scalper = bot::scalper::ScalperBot::new(redis_conn.clone()).await?;

    let mut graph = graph::Graph::new();

    // 4️⃣ Poll loop
    let mut interval = time::interval(Duration::from_secs(cfg.poll_interval_secs));

    // let compounded_capital = Graph::compound_from_redis(redis_conn.clone(), cfg.margin).await?;
    // info!("compounded_capital -> {:?}", compounded_capital);

    Graph::prepare_cumulative_weekly_monthly(&mut graph, redis_conn.clone()).await?;

    loop {
        interval.tick().await;

        match exchange.get_current_price().await {
            Ok(price) => {
                info!("Price = {:.2}", price,);
                if let Err(e) = bot.run_cycle(price, exchange.as_ref(), &mut cfg).await {
                    eprintln!("Error during cycle: {e}");
                }

                if let Err(e) = scalper
                    .run_scalper_bot(price, exchange.as_ref(), &mut cfg)
                    .await
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
