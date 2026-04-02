use std::error::Error;
use std::sync::Arc;

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

    // 2️⃣ Create exchange instance based on EXCHANGE env var
    let exchange: Arc<dyn crate::exchange::Exchange> = match cfg.exchange {
        ExchangeType::Bitunix => Arc::new(BitunixExchange::new(&cfg)),
        ExchangeType::Bitget => Arc::new(HttpExchange {
            client: reqwest::Client::new(),
            symbol: cfg.symbol.clone(),
            redis_conn: redis_conn.clone(),
        }),
    };

    // 3️⃣ Bot state
    let mut bot = bot::Bot::new(redis_conn.clone(), &cfg).await?;

    if cfg.use_smc_indicator {
        let smc_conn = redis_conn.clone();
        let smc_config = cfg.clone();
        let _smc_handle = tokio::spawn(async move {
            trackers::smart_money_concepts::smc_loop(smc_conn, smc_config).await;
        });
    }

    if cfg.use_ichimoku_indicator {
        let ichimoku_conn = redis_conn.clone();
        let _tracker_ichimoku = tokio::spawn(async move {
            if let Err(e) = trackers::ichimoku::ichimoku_loop(ichimoku_conn).await {
                log::error!("Ichimoku tracker error: {e}");
            }
        });
    }

    // 4️⃣ Spawn API server
    let api_conn = redis_conn.clone();
    let _api_handle = tokio::spawn(async move {
        let app = api::create_router(api_conn);
        let listener = tokio::net::TcpListener::bind("0.0.0.0:4545")
            .await
            .expect("Failed to bind API server");

        info!("API server listening on http://0.0.0.0:4545");

        if let Err(e) = axum::serve(listener, app).await {
            log::error!("API server error: {e}");
        }
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
