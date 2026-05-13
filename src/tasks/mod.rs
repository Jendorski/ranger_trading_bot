use std::sync::Arc;

use log::info;
use tokio::task::JoinSet;

use crate::api;
use crate::config::Config;
use crate::helper::{
    TRADING_BOT_RSI_SNAPSHOT_1D, TRADING_BOT_RSI_SNAPSHOT_1H,
    TRADING_BOT_RSI_SNAPSHOT_15M, TRADING_BOT_RSI_SNAPSHOT_3D,
    TRADING_BOT_RSI_SNAPSHOT_4H,
};
use crate::trackers;
use crate::trackers::smart_money_concepts::Bar;

/// Loads a seed candle file into a `Vec<Bar>` for use by RSI tracker loops.
///
/// Called once at startup inside `spawn_blocking` — never on a hot tick path.
/// Returns an empty vec (and logs a warning) if the file is missing or corrupt;
/// the RSI loops will then run in live-only mode until seeds are regenerated.
fn load_seed_bars(path: &'static str) -> Vec<Bar> {
    match crate::data::aggregator::read_candles_json(path) {
        Ok(candles) => candles
            .into_iter()
            .map(|c| Bar {
                time: c.timestamp,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                volume: Some(c.volume),
                volume_quote: None,
            })
            .collect(),
        Err(e) => {
            log::warn!("[seeds] Could not load '{path}': {e} — RSI loop will use live-only data");
            Vec::new()
        }
    }
}

/// Spawns all background tasks and returns a [`JoinSet`] that owns them.
///
/// The caller must drive the `JoinSet` — dropping it aborts every task inside.
/// Pass it to a supervisor loop (see `main.rs`) so unexpected exits are logged.
pub async fn spawn_background_tasks(
    redis_conn: redis::aio::MultiplexedConnection,
    cfg: &Config,
    http: Arc<reqwest::Client>,
) -> JoinSet<()> {
    let symbol: Arc<str> = Arc::from(cfg.symbol.as_str());

    // Load all seed files in parallel at startup. Each read is blocking I/O so
    // it runs in spawn_blocking; tokio::join! drives them concurrently.
    // Seeds are immutable after generation — wrap in Arc so every loop tick
    // pays only a refcount clone, not a disk read.
    let (s1w, s2w, s3d, s1d, s4h, s1h, s15m) = tokio::join!(
        tokio::task::spawn_blocking(|| load_seed_bars("data/candles_1w.json")),
        tokio::task::spawn_blocking(|| load_seed_bars("data/candles_2w.json")),
        tokio::task::spawn_blocking(|| load_seed_bars("data/candles_3d.json")),
        tokio::task::spawn_blocking(|| load_seed_bars("data/candles_1d.json")),
        tokio::task::spawn_blocking(|| load_seed_bars("data/candles_4h.json")),
        tokio::task::spawn_blocking(|| load_seed_bars("data/candles_1h.json")),
        tokio::task::spawn_blocking(|| load_seed_bars("data/candles_15m.json")),
    );

    let seed_1w  = Arc::new(s1w.unwrap_or_default());
    let seed_2w  = Arc::new(s2w.unwrap_or_default());
    let seed_3d  = Arc::new(s3d.unwrap_or_default());
    let seed_1d  = Arc::new(s1d.unwrap_or_default());
    let seed_4h  = Arc::new(s4h.unwrap_or_default());
    let seed_1h  = Arc::new(s1h.unwrap_or_default());
    let seed_15m = Arc::new(s15m.unwrap_or_default());

    info!(
        "[seeds] Loaded: 1W={} 2W={} 3D={} 1D={} 4H={} 1H={} 15m={} bars",
        seed_1w.len(), seed_2w.len(), seed_3d.len(), seed_1d.len(),
        seed_4h.len(), seed_1h.len(), seed_15m.len(),
    );

    let mut task_set: JoinSet<()> = JoinSet::new();

    if cfg.use_smc_indicator {
        let conn = redis_conn.clone();
        let smc_config = cfg.clone();
        task_set.spawn(async move {
            trackers::smart_money_concepts::smc_loop(conn, smc_config).await;
        });
    }

    if cfg.use_ichimoku_indicator {
        let (conn, h, sym, seeds) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol), Arc::clone(&seed_1w));
        task_set.spawn(async move {
            trackers::ichimoku::ichimoku_loop(conn, h, sym, seeds, 14400).await;
        });
    }

    // 4H VRVP — 500 candles (~83 days of structure); 100 bins; refresh every 30 min
    let (conn, h, sym) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol));
    task_set.spawn(async move {
        trackers::visible_range_volume_profile::vrvp_loop(conn, h, sym, "4H", 500, 100, 1800).await;
    });

    // 1D VRVP — 365 candles (~1 year of daily structure); 75 bins; refresh every 2 hours
    let (conn, h, sym) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol));
    task_set.spawn(async move {
        trackers::visible_range_volume_profile::vrvp_loop(conn, h, sym, "1D", 365, 75, 7200).await;
    });

    // 1W VRVP — 52 candles (~1 year of weekly structure); 60 bins; refresh every 4 hours
    let (conn, h, sym) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol));
    task_set.spawn(async move {
        trackers::visible_range_volume_profile::vrvp_loop(conn, h, sym, "1W", 52, 60, 14400).await;
    });

    // Weekly RSI regime gate — refresh every 4 hours (matches 1W VRVP cadence)
    let (conn, h, sym, seeds) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol), Arc::clone(&seed_1w));
    task_set.spawn(async move {
        trackers::rsi_regime_tracker::rsi_regime_loop(conn, h, sym, seeds, 14400).await;
    });

    // 2W RSI — biweekly macro context; aggregated from live 1W candles; refresh every 12 hours
    let (conn, h, sym, seeds) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol), Arc::clone(&seed_2w));
    task_set.spawn(async move {
        trackers::rsi_regime_tracker::rsi_2w_loop(conn, h, sym, seeds, 43200).await;
    });

    // 3D RSI — structural confirmation layer; refresh every 6 hours
    let (conn, h, sym, seeds) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol), Arc::clone(&seed_3d));
    task_set.spawn(async move {
        trackers::rsi_regime_tracker::rsi_snapshot_loop(
            conn, h, sym, seeds, "3D", 100, 21600,
            TRADING_BOT_RSI_SNAPSHOT_3D,
        ).await;
    });

    // 1D RSI — trend confirmation; refresh every 2 hours
    let (conn, h, sym, seeds) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol), Arc::clone(&seed_1d));
    task_set.spawn(async move {
        trackers::rsi_regime_tracker::rsi_snapshot_loop(
            conn, h, sym, seeds, "1D", 200, 7200,
            TRADING_BOT_RSI_SNAPSHOT_1D,
        ).await;
    });

    // 4H RSI — primary entry strength gate (Archetype 2); refresh every 15 minutes
    let (conn, h, sym, seeds) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol), Arc::clone(&seed_4h));
    task_set.spawn(async move {
        trackers::rsi_regime_tracker::rsi_snapshot_loop(
            conn, h, sym, seeds, "4H", 300, 900,
            TRADING_BOT_RSI_SNAPSHOT_4H,
        ).await;
    });

    // 1H RSI — entry refinement; refresh every 5 minutes
    let (conn, h, sym, seeds) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol), Arc::clone(&seed_1h));
    task_set.spawn(async move {
        trackers::rsi_regime_tracker::rsi_snapshot_loop(
            conn, h, sym, seeds, "1H", 200, 300,
            TRADING_BOT_RSI_SNAPSHOT_1H,
        ).await;
    });

    // 15m RSI — short-term entry timing; refresh every minute
    let (conn, h, sym, seeds) = (redis_conn.clone(), Arc::clone(&http), Arc::clone(&symbol), Arc::clone(&seed_15m));
    task_set.spawn(async move {
        trackers::rsi_regime_tracker::rsi_snapshot_loop(
            conn, h, sym, seeds, "15m", 200, 60,
            TRADING_BOT_RSI_SNAPSHOT_15M,
        ).await;
    });

    // MacroTracker — 5 macro resistance levels; refresh every 4 hours
    let (conn, h, sym, s1d, s1w, s2w) = (
        redis_conn.clone(),
        Arc::clone(&http),
        Arc::clone(&symbol),
        Arc::clone(&seed_1d),
        Arc::clone(&seed_1w),
        Arc::clone(&seed_2w),
    );
    task_set.spawn(async move {
        crate::regime::macro_tracker_loop(conn, h, sym, s1d, s1w, s2w, 14400).await;
    });

    // 4H RSI divergence — Strength gate; 300 candles; refresh every 15 minutes
    let (conn, h, sym, s4h) = (
        redis_conn.clone(),
        Arc::clone(&http),
        Arc::clone(&symbol),
        Arc::clone(&seed_4h),
    );
    task_set.spawn(async move {
        trackers::rsi_divergence_indicator::rsi_div_loop(
            conn, h, sym, s4h, "4H", "300", "trading_bot:rsi_div:4H", 900,
        )
        .await;
    });

    // 1D RSI divergence — higher-TF confirmation; 200 candles; refresh every 2 hours
    let (conn, h, sym, s1d_div) = (
        redis_conn.clone(),
        Arc::clone(&http),
        Arc::clone(&symbol),
        Arc::clone(&seed_1d),
    );
    task_set.spawn(async move {
        trackers::rsi_divergence_indicator::rsi_div_loop(
            conn, h, sym, s1d_div, "1D", "200", "trading_bot:rsi_div:1D", 7200,
        )
        .await;
    });

    // GaussianChannel 3D — macro regime filter (BullIntact / Transitioning / BearIntact); refresh every 3 hours
    let (conn, h, sym, s3d) = (
        redis_conn.clone(),
        Arc::clone(&http),
        Arc::clone(&symbol),
        Arc::clone(&seed_3d),
    );
    task_set.spawn(async move {
        crate::regime::gaussian_3d_loop(conn, h, sym, s3d, 10800).await;
    });

    task_set.spawn(async move {
        let app = api::create_router(redis_conn);
        let listener = tokio::net::TcpListener::bind("0.0.0.0:4545")
            .await
            .expect("Failed to bind API server");

        info!("API server listening on http://0.0.0.0:4545");

        if let Err(e) = axum::serve(listener, app).await {
            log::error!("API server error: {e}");
        }
    });

    task_set
}
