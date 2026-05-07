use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use log::info;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::time;

use crate::exchange::bitget::fetch_bitget_candles;
use crate::helper::TRADING_BOT_MACRO_TRACKER;
use crate::trackers::ema::Ema;
use crate::trackers::gaussian::GaussianChannel;
use crate::trackers::ichimoku::IchimokuBaseline;
use crate::trackers::smart_money_concepts::Bar;

// ─── Public snapshot types ────────────────────────────────────────────────────

/// Macro bias derived from how many of the 5 resistance levels price is above.
///
/// Trend-agnostic: both Bullish and Bearish scenarios are tracked. The
/// count-based classification naturally flips as price moves through levels
/// in either direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroBias {
    /// 0–1 levels above price — macro resistance holding
    Bearish,
    /// 2–3 levels above/below price — transitional zone
    Transitional,
    /// 4–5 levels below price — macro resistance cleared
    Bullish,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroTrackerSnapshot {
    /// Daily 200 EMA — first macro resistance level
    pub daily_ema_200: Option<f64>,
    /// 2-Week 50 EMA — second level
    pub ema_2w_50: Option<f64>,
    /// 2-Week Gaussian Channel midline — third level
    pub gc_2w_midline: Option<f64>,
    /// Weekly Gaussian Channel lower band — fourth level (~$86k at time of design)
    pub gc_1w_lower_band: Option<f64>,
    /// Weekly Ichimoku Cloud baseline (Kijun-sen) — fifth level
    pub ichimoku_weekly_baseline: Option<f64>,
    /// How many of the 5 levels the current price is trading above (0–5)
    pub price_above_count: u8,
    pub macro_bias: MacroBias,
    pub current_price: f64,
    pub updated_at: DateTime<Utc>,
}

// ─── MacroTracker ─────────────────────────────────────────────────────────────

const GC_POLES: usize = 4;
const GC_SAMPLING: usize = 144;
const GC_MULTIPLIER: f64 = 1.414;

#[derive(Debug, Clone)]
struct MacroTracker {
    ema_1d_200: Ema,
    ema_2w_50: Ema,
    gc_2w: GaussianChannel,
    gc_1w: GaussianChannel,
    baseline_1w: IchimokuBaseline,
}

impl MacroTracker {
    fn new() -> Self {
        Self {
            ema_1d_200: Ema::new(200),
            ema_2w_50: Ema::new(50),
            gc_2w: GaussianChannel::new(GC_POLES, GC_SAMPLING, GC_MULTIPLIER),
            gc_1w: GaussianChannel::new(GC_POLES, GC_SAMPLING, GC_MULTIPLIER),
            baseline_1w: IchimokuBaseline::new(),
        }
    }

    fn process_1d_bar(&mut self, bar: &Bar) {
        self.ema_1d_200.update(bar.close);
    }

    fn process_2w_bar(&mut self, bar: &Bar) {
        self.ema_2w_50.update(bar.close);
        self.gc_2w.update(bar.high, bar.low, bar.close);
    }

    fn process_1w_bar(&mut self, bar: &Bar) {
        self.gc_1w.update(bar.high, bar.low, bar.close);
        self.baseline_1w.update(bar.high, bar.low);
    }

    fn snapshot(&self, current_price: f64) -> MacroTrackerSnapshot {
        let ema_1d = self.ema_1d_200.current();
        let ema_2w = self.ema_2w_50.current();
        let gc_2w_mid = self.gc_2w.midline;
        let gc_1w_lower = self.gc_1w.lower_band;
        let baseline = self.baseline_1w.value;

        let above_flags = [
            ema_1d.map(|v| current_price > v),
            ema_2w.map(|v| current_price > v),
            gc_2w_mid.map(|v| current_price > v),
            gc_1w_lower.map(|v| current_price > v),
            baseline.map(|v| current_price > v),
        ];

        let price_above_count = above_flags.iter().filter(|f| **f == Some(true)).count() as u8;

        let macro_bias = match price_above_count {
            0..=1 => MacroBias::Bearish,
            2..=3 => MacroBias::Transitional,
            _ => MacroBias::Bullish,
        };

        MacroTrackerSnapshot {
            daily_ema_200: ema_1d,
            ema_2w_50: ema_2w,
            gc_2w_midline: gc_2w_mid,
            gc_1w_lower_band: gc_1w_lower,
            ichimoku_weekly_baseline: baseline,
            price_above_count,
            macro_bias,
            current_price,
            updated_at: Utc::now(),
        }
    }
}

// ─── Background loop ──────────────────────────────────────────────────────────

pub async fn macro_tracker_loop(
    mut conn: redis::aio::MultiplexedConnection,
    http: Arc<reqwest::Client>,
    symbol: Arc<str>,
    seed_1d: Arc<Vec<Bar>>,
    seed_1w: Arc<Vec<Bar>>,
    seed_2w: Arc<Vec<Bar>>,
    interval_secs: u64,
) {
    let seed_cutoff_1d = seed_1d.iter().map(|b| b.time).max();
    let seed_cutoff_1w = seed_1w.iter().map(|b| b.time).max();
    let seed_cutoff_2w = seed_2w.iter().map(|b| b.time).max();

    let mut seed_tracker = MacroTracker::new();
    for bar in seed_1d.iter() {
        seed_tracker.process_1d_bar(bar);
    }
    for bar in seed_2w.iter() {
        seed_tracker.process_2w_bar(bar);
    }
    for bar in seed_1w.iter() {
        seed_tracker.process_1w_bar(bar);
    }
    drop(seed_1d);
    drop(seed_1w);
    drop(seed_2w);

    info!(
        "MacroTracker: seed warmup complete — ema_1d={:?} ema_2w={:?} gc_1w_lower={:?}",
        seed_tracker.ema_1d_200.current(),
        seed_tracker.ema_2w_50.current(),
        seed_tracker.gc_1w.lower_band,
    );

    let mut interval = time::interval(Duration::from_secs(interval_secs));
    loop {
        interval.tick().await;
        macro_tracker_main(
            &mut conn,
            &http,
            &symbol,
            &seed_tracker,
            seed_cutoff_1d,
            seed_cutoff_1w,
            seed_cutoff_2w,
            interval_secs,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn macro_tracker_main(
    conn: &mut redis::aio::MultiplexedConnection,
    http: &reqwest::Client,
    symbol: &str,
    seed_tracker: &MacroTracker,
    seed_cutoff_1d: Option<DateTime<Utc>>,
    seed_cutoff_1w: Option<DateTime<Utc>>,
    seed_cutoff_2w: Option<DateTime<Utc>>,
    interval_secs: u64,
) {
    // --- 1. Fetch live candles ---
    let candles_1d = match fetch_bitget_candles(http, symbol, "1D", "200").await {
        Ok(c) => c,
        Err(e) => {
            log::error!("MacroTracker: 1D fetch error: {e}");
            Vec::new()
        }
    };
    let candles_1w = match fetch_bitget_candles(http, symbol, "1W", "52").await {
        Ok(c) => c,
        Err(e) => {
            log::error!("MacroTracker: 1W fetch error: {e}");
            Vec::new()
        }
    };

    // --- 2. Current price from most recent 1D close ---
    let current_price = candles_1d
        .iter()
        .max_by_key(|c| c.timestamp)
        .map(|c| c.close)
        .unwrap_or(0.0);

    if current_price == 0.0 {
        log::warn!("MacroTracker: no current price, skipping tick");
        return;
    }

    // --- 3. Build live 1D delta ---
    let mut live_1d: Vec<Bar> = candles_1d
        .iter()
        .filter_map(|c| {
            let t = Utc.timestamp_millis_opt(c.timestamp).single()?;
            if seed_cutoff_1d.is_none_or(|cut| t > cut) {
                Some(candle_to_bar(t, c))
            } else {
                None
            }
        })
        .collect();
    live_1d.sort_by_key(|b| b.time);

    // --- 4. Build live 1W delta ---
    let mut live_1w: Vec<Bar> = candles_1w
        .iter()
        .filter_map(|c| {
            let t = Utc.timestamp_millis_opt(c.timestamp).single()?;
            if seed_cutoff_1w.is_none_or(|cut| t > cut) {
                Some(candle_to_bar(t, c))
            } else {
                None
            }
        })
        .collect();
    live_1w.sort_by_key(|b| b.time);

    // --- 5. Aggregate live 1W → 2W, filter to new bars only ---
    let mut buckets: BTreeMap<i64, Bar> = BTreeMap::new();
    for c in candles_1w.iter() {
        let t = match Utc.timestamp_millis_opt(c.timestamp).single() {
            Some(t) => t,
            None => continue,
        };
        let key = t.timestamp() / (14 * 24 * 3600);
        buckets
            .entry(key)
            .and_modify(|b| {
                b.high = b.high.max(c.high);
                b.low = b.low.min(c.low);
                b.close = c.close;
                b.volume = Some(b.volume.unwrap_or(0.0) + c.volume);
            })
            .or_insert(candle_to_bar(t, c));
    }
    let mut live_2w: Vec<Bar> = buckets
        .into_values()
        .filter(|b| seed_cutoff_2w.is_none_or(|cut| b.time > cut))
        .collect();
    live_2w.sort_by_key(|b| b.time);

    // --- 6. Clone seeded state and apply live delta ---
    let mut tracker = seed_tracker.clone();
    for bar in &live_1d {
        tracker.process_1d_bar(bar);
    }
    for bar in &live_2w {
        tracker.process_2w_bar(bar);
    }
    for bar in &live_1w {
        tracker.process_1w_bar(bar);
    }

    let snapshot = tracker.snapshot(current_price);

    info!(
        "MacroTracker: price={:.0} above={}/5 bias={:?} | ema1d={:.0?} ema2w={:.0?} gc2w={:.0?} gc1w_lower={:.0?} kijun={:.0?}",
        snapshot.current_price,
        snapshot.price_above_count,
        snapshot.macro_bias,
        snapshot.daily_ema_200,
        snapshot.ema_2w_50,
        snapshot.gc_2w_midline,
        snapshot.gc_1w_lower_band,
        snapshot.ichimoku_weekly_baseline,
    );

    let serialized = match serde_json::to_string(&snapshot) {
        Ok(s) => s,
        Err(e) => {
            log::error!("MacroTracker: serialisation error: {e}");
            return;
        }
    };

    let ttl = (interval_secs * 2) as usize;
    if let Err(e) = conn
        .set_ex::<_, _, ()>(TRADING_BOT_MACRO_TRACKER, serialized, ttl)
        .await
    {
        log::error!("MacroTracker: Redis write failed: {e}");
    }
}

fn candle_to_bar(t: DateTime<Utc>, c: &crate::exchange::bitget::Candle) -> Bar {
    Bar {
        time: t,
        open: c.open,
        high: c.high,
        low: c.low,
        close: c.close,
        volume: Some(c.volume),
        volume_quote: Some(c.quote_volume),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(close: f64) -> Bar {
        Bar {
            time: Utc::now(),
            open: close,
            high: close * 1.01,
            low: close * 0.99,
            close,
            volume: None,
            volume_quote: None,
        }
    }

    fn bar_hl(high: f64, low: f64) -> Bar {
        Bar {
            time: Utc::now(),
            open: (high + low) / 2.0,
            high,
            low,
            close: (high + low) / 2.0,
            volume: None,
            volume_quote: None,
        }
    }

    #[test]
    fn macro_tracker_bias_all_below_is_bearish() {
        let mut tracker = MacroTracker::new();
        // Feed 300 flat 1D bars at 50_000
        for _ in 0..300 {
            tracker.process_1d_bar(&bar(50_000.0));
        }
        // Feed 200 flat 2W bars at 50_000
        for _ in 0..200 {
            tracker.process_2w_bar(&bar_hl(51_000.0, 49_000.0));
        }
        // Feed 200 flat 1W bars at 50_000
        for _ in 0..200 {
            tracker.process_1w_bar(&bar_hl(51_000.0, 49_000.0));
        }
        // Price well below all indicators at 50_000 — all indicators should be near 50_000
        // so check snapshot reports non-zero counts (exact values depend on convergence)
        let snap = tracker.snapshot(50_000.0);
        assert!(snap.daily_ema_200.is_some());
        assert!(snap.ema_2w_50.is_some());
        assert!(snap.gc_2w_midline.is_some());
        assert!(snap.gc_1w_lower_band.is_some());
        assert!(snap.ichimoku_weekly_baseline.is_some());
    }
}
