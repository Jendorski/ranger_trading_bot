//! # RSI Divergence Indicator
//!
//! Detects the four classical RSI divergence types (Regular Bullish/Bearish,
//! Hidden Bullish/Bearish) by feeding bars in chronological order to
//! [`RsiDivEngine::process_bar`].
//!
//! ## Design: RSI-pivot detection (not price-pivot)
//!
//! **Pivots are found on RSI, not on price.** The engine identifies bars where
//! the RSI value is a local minimum or maximum within the `[lb_left, lb_right]`
//! window. Price (`low`/`high`) is then *read at that bar*. This matches the
//! reference Pine Script (`ta.pivotlow(osc, lbL, lbR)`), but differs from
//! classic textbook methodology, which finds price swing pivots first and reads
//! RSI there.
//!
//! **Trading implication for BTC:** On wick-heavy candles, RSI can spike to an
//! extreme on a bar that closes mid-range. That bar becomes an RSI pivot, and
//! its recorded `pivot_price` is the wick `low`/`high` — not a structural swing.
//! Downstream consumers must treat `pivot_price` as the price *at the RSI pivot
//! bar*, not necessarily a meaningful price structure level.
//!
//! For SMC confluence, **never route raw `RsiDivEvent`s directly to order
//! placement.** Filter against active `SmcEngine` zones first: a regular bearish
//! divergence inside a supply zone is a high-probability short; the same signal
//! in open space without zone context is noise.
//!
//! ## Confirmation lag
//!
//! All signals are confirmed `lb_right` bars **after** the pivot forms
//! (matching Pine Script's `offset = -lbR`). No repainting, but there is lag:
//! - `lb_right = 5` on a 1m chart → 5-minute lag
//! - `lb_right = 5` on a 5m chart → 25-minute lag
//!
//! ## Timeframe guidance for `default_params()`
//!
//! [`RsiDivEngine::default_params`] uses `range_upper = 60`, which means:
//!
//! | Timeframe | Max divergence window |
//! |-----------|----------------------|
//! | 1m        | 1 hour               |
//! | 5m        | 5 hours              |
//! | 15m       | 15 hours             |
//! | 1h        | 2.5 days             |
//!
//! Pass explicit parameters via [`RsiDivEngine::new`] when the timeframe
//! differs from 5m/15m.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use log::info;
use redis::{aio::MultiplexedConnection, AsyncCommands};
use serde::{Deserialize, Serialize};
use tokio::time;

use crate::exchange::bitget::fetch_bitget_candles;
use super::rsi_core::RsiCore;
use super::smart_money_concepts::Bar;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Divergence signals emitted when a confirmed pivot divergence is detected.
///
/// All events are confirmed `lb_right` bars *after* the actual pivot forms
/// (matching Pine Script's `offset = -lbR` behavior — no repainting).
///
/// ## Magnitude fields
///
/// Every variant carries both pivots so downstream logic can grade signal
/// strength without re-deriving:
///
/// - `rsi_delta` — absolute RSI difference between the two pivots.
///   Larger values indicate stronger divergence. Filter example: `rsi_delta > 5.0`.
/// - `price_delta_pct` — percentage price move between the two pivots
///   (always positive). Larger values indicate a more significant price swing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RsiDivEvent {
    /// Price: Lower Low  |  RSI: Higher Low  →  potential reversal up
    RegularBullish {
        /// RSI value at the current (newer) pivot
        rsi_value: f64,
        /// Price (`low`) at the current pivot
        pivot_price: f64,
        /// RSI value at the previous (older) pivot
        prev_rsi_value: f64,
        /// Price (`low`) at the previous pivot
        prev_pivot_price: f64,
        /// `|rsi_value - prev_rsi_value|` — magnitude of the RSI divergence
        rsi_delta: f64,
        /// `|(pivot_price - prev_pivot_price) / prev_pivot_price| * 100` — % price swing
        price_delta_pct: f64,
        time: DateTime<Utc>,
        bar_index: usize,
    },
    /// Price: Higher Low  |  RSI: Lower Low  →  uptrend continuation
    HiddenBullish {
        rsi_value: f64,
        pivot_price: f64,
        prev_rsi_value: f64,
        prev_pivot_price: f64,
        rsi_delta: f64,
        price_delta_pct: f64,
        time: DateTime<Utc>,
        bar_index: usize,
    },
    /// Price: Higher High  |  RSI: Lower High  →  potential reversal down
    RegularBearish {
        rsi_value: f64,
        pivot_price: f64,
        prev_rsi_value: f64,
        prev_pivot_price: f64,
        rsi_delta: f64,
        price_delta_pct: f64,
        time: DateTime<Utc>,
        bar_index: usize,
    },
    /// Price: Lower High  |  RSI: Higher High  →  downtrend continuation
    HiddenBearish {
        rsi_value: f64,
        pivot_price: f64,
        prev_rsi_value: f64,
        prev_pivot_price: f64,
        rsi_delta: f64,
        price_delta_pct: f64,
        time: DateTime<Utc>,
        bar_index: usize,
    },
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// One slot in the pivot-detection rolling window.
///
/// Bundling all three fields into a single struct means the window is a single
/// `VecDeque<WindowEntry>` — push and pop are one call each, and the three
/// values can never fall out of sync.
#[derive(Debug, Clone)]
struct WindowEntry {
    rsi: f64,
    bar: Bar,
    global_idx: usize,
}

#[derive(Debug, Clone)]
struct PivotRecord {
    rsi: f64,
    /// `low` for pivot lows, `high` for pivot highs
    price: f64,
    bar_index: usize,
    time: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Maximum number of past pivots retained per direction for divergence lookup.
///
/// Keeping more than one pivot prevents the single-pivot-replacement bug where
/// Pivot B (within range of A, wrong direction) overwrites A, causing A→C
/// divergences to be silently missed when C arrives later. A buffer of 3 is
/// sufficient for all practical divergence patterns.
const PIVOT_MEMORY: usize = 3;

/// RSI Divergence Engine.
///
/// Feed bars in chronological order via [`RsiDivEngine::process_bar`].
/// Returns divergence events once enough bars have accumulated.
///
/// Default Pine Script parameters:
/// - `len = 14`, `lb_left = 5`, `lb_right = 5`
/// - `range_lower = 5`, `range_upper = 60`
#[derive(Clone)]
pub struct RsiDivEngine {
    // Config
    lb_left: usize,
    lb_right: usize,
    range_lower: usize,
    range_upper: usize,

    rsi: RsiCore,

    /// Rolling window of size `lb_left + lb_right + 1`.
    /// Pivot detection reads RSI and price from this single deque; all three
    /// values are always in sync because they travel as one `WindowEntry`.
    win: VecDeque<WindowEntry>,

    /// Ring-buffer of recent confirmed pivot lows (oldest at front, newest at back).
    /// Pivots that are permanently out of `range_upper` are pruned on each new pivot.
    pivot_lows: VecDeque<PivotRecord>,
    /// Ring-buffer of recent confirmed pivot highs.
    pivot_highs: VecDeque<PivotRecord>,

    // RSI zone filters — suppresses low-probability signals when RSI is in the
    // wrong part of its range.
    /// Bullish signals (Regular + Hidden) are suppressed when `cand_rsi >= bull_rsi_max`.
    /// Typical value: `50.0`. `None` disables the filter.
    bull_rsi_max: Option<f64>,
    /// Bearish signals (Regular + Hidden) are suppressed when `cand_rsi <= bear_rsi_min`.
    /// Typical value: `50.0`. `None` disables the filter.
    bear_rsi_min: Option<f64>,

    global_bar_idx: usize,
}

impl RsiDivEngine {
    /// Create a new engine with explicit parameters.
    pub fn new(
        len: usize,
        lb_left: usize,
        lb_right: usize,
        range_lower: usize,
        range_upper: usize,
    ) -> Self {
        let win_size = lb_left + lb_right + 1;
        Self {
            rsi: RsiCore::new(len),
            lb_left,
            lb_right,
            range_lower,
            range_upper,
            win: VecDeque::with_capacity(win_size),
            pivot_lows: VecDeque::with_capacity(PIVOT_MEMORY),
            pivot_highs: VecDeque::with_capacity(PIVOT_MEMORY),
            bull_rsi_max: None,
            bear_rsi_min: None,
            global_bar_idx: 0,
        }
    }

    /// Convenience constructor using Pine Script defaults.
    pub fn default_params() -> Self {
        Self::new(14, 5, 5, 5, 60)
    }

    /// Attach RSI zone filters to suppress low-probability signals.
    ///
    /// - `bull_max`: bullish signals (Regular + Hidden) are suppressed when the
    ///   pivot RSI is **≥ bull_max**. Typical value: `50.0` — only fire bullish
    ///   signals when RSI is in oversold/neutral territory.
    /// - `bear_min`: bearish signals (Regular + Hidden) are suppressed when the
    ///   pivot RSI is **≤ bear_min**. Typical value: `50.0` — only fire bearish
    ///   signals when RSI is in overbought/neutral territory.
    ///
    /// Returns `self` for builder-style chaining:
    /// ```ignore
    /// let eng = RsiDivEngine::new(14, 5, 5, 5, 60)
    ///     .with_rsi_filter(Some(50.0), Some(50.0));
    /// ```
    pub fn with_rsi_filter(mut self, bull_max: Option<f64>, bear_min: Option<f64>) -> Self {
        self.bull_rsi_max = bull_max;
        self.bear_rsi_min = bear_min;
        self
    }

    // -----------------------------------------------------------------------
    // Public interface
    // -----------------------------------------------------------------------

    /// Process one bar (chronological order). Returns any divergence events
    /// that were confirmed at this bar.
    pub fn process_bar(&mut self, bar: Bar) -> Vec<RsiDivEvent> {
        let bar_idx = self.global_bar_idx;
        self.global_bar_idx += 1;

        let Some(rsi) = self.rsi.update(bar.close) else {
            return Vec::new();
        };

        let win_size = self.lb_left + self.lb_right + 1;

        // Maintain fixed-size rolling window — one push/pop keeps all fields in sync.
        if self.win.len() == win_size {
            self.win.pop_front();
        }
        self.win.push_back(WindowEntry { rsi, bar, global_idx: bar_idx });

        if self.win.len() < win_size {
            return Vec::new();
        }

        let mut events = Vec::new();

        // Candidate pivot sits at `lb_left` (the "center" of the window)
        let ci = self.lb_left;
        let cand_rsi = self.win[ci].rsi;
        let cand_bar = self.win[ci].bar.clone();
        let cand_global = self.win[ci].global_idx;

        // ----------------------------------------------------------------
        // Pivot Low: RSI at ci is strictly less than every neighbor
        // ----------------------------------------------------------------
        let is_pivot_low = (0..ci).all(|i| self.win[i].rsi > cand_rsi)
            && (ci + 1..win_size).all(|i| self.win[i].rsi > cand_rsi);

        if is_pivot_low {
            // Drop pivots that are now permanently out of range — any future
            // pivot will be even farther away, so they can never match again.
            self.pivot_lows
                .retain(|p| cand_global.saturating_sub(p.bar_index) <= self.range_upper);

            // RSI zone filter: skip bullish signals when RSI is too high.
            let bull_zone_ok = self.bull_rsi_max.is_none_or(|max| cand_rsi < max);

            // Check every stored pivot in the valid distance window.
            for prev in &self.pivot_lows {
                let dist = cand_global.saturating_sub(prev.bar_index);
                if dist >= self.range_lower && bull_zone_ok {
                    // Regular Bullish: price Lower Low + RSI Higher Low
                    if cand_bar.low < prev.price && cand_rsi > prev.rsi {
                        events.push(RsiDivEvent::RegularBullish {
                            rsi_value: cand_rsi,
                            pivot_price: cand_bar.low,
                            prev_rsi_value: prev.rsi,
                            prev_pivot_price: prev.price,
                            rsi_delta: (cand_rsi - prev.rsi).abs(),
                            price_delta_pct: ((cand_bar.low - prev.price) / prev.price).abs() * 100.0,
                            time: cand_bar.time,
                            bar_index: cand_global,
                        });
                    }
                    // Hidden Bullish: price Higher Low + RSI Lower Low
                    if cand_bar.low > prev.price && cand_rsi < prev.rsi {
                        events.push(RsiDivEvent::HiddenBullish {
                            rsi_value: cand_rsi,
                            pivot_price: cand_bar.low,
                            prev_rsi_value: prev.rsi,
                            prev_pivot_price: prev.price,
                            rsi_delta: (cand_rsi - prev.rsi).abs(),
                            price_delta_pct: ((cand_bar.low - prev.price) / prev.price).abs() * 100.0,
                            time: cand_bar.time,
                            bar_index: cand_global,
                        });
                    }
                }
            }

            // Push the new pivot; evict the oldest if the cap is exceeded.
            self.pivot_lows.push_back(PivotRecord {
                rsi: cand_rsi,
                price: cand_bar.low,
                bar_index: cand_global,
                time: cand_bar.time,
            });
            if self.pivot_lows.len() > PIVOT_MEMORY {
                self.pivot_lows.pop_front();
            }
        }

        // ----------------------------------------------------------------
        // Pivot High: RSI at ci is strictly greater than every neighbor
        // ----------------------------------------------------------------
        let is_pivot_high = (0..ci).all(|i| self.win[i].rsi < cand_rsi)
            && (ci + 1..win_size).all(|i| self.win[i].rsi < cand_rsi);

        if is_pivot_high {
            self.pivot_highs
                .retain(|p| cand_global.saturating_sub(p.bar_index) <= self.range_upper);

            // RSI zone filter: skip bearish signals when RSI is too low.
            let bear_zone_ok = self.bear_rsi_min.is_none_or(|min| cand_rsi > min);

            for prev in &self.pivot_highs {
                let dist = cand_global.saturating_sub(prev.bar_index);
                if dist >= self.range_lower && bear_zone_ok {
                    // Regular Bearish: price Higher High + RSI Lower High
                    if cand_bar.high > prev.price && cand_rsi < prev.rsi {
                        events.push(RsiDivEvent::RegularBearish {
                            rsi_value: cand_rsi,
                            pivot_price: cand_bar.high,
                            prev_rsi_value: prev.rsi,
                            prev_pivot_price: prev.price,
                            rsi_delta: (cand_rsi - prev.rsi).abs(),
                            price_delta_pct: ((cand_bar.high - prev.price) / prev.price).abs() * 100.0,
                            time: cand_bar.time,
                            bar_index: cand_global,
                        });
                    }
                    // Hidden Bearish: price Lower High + RSI Higher High
                    if cand_bar.high < prev.price && cand_rsi > prev.rsi {
                        events.push(RsiDivEvent::HiddenBearish {
                            rsi_value: cand_rsi,
                            pivot_price: cand_bar.high,
                            prev_rsi_value: prev.rsi,
                            prev_pivot_price: prev.price,
                            rsi_delta: (cand_rsi - prev.rsi).abs(),
                            price_delta_pct: ((cand_bar.high - prev.price) / prev.price).abs() * 100.0,
                            time: cand_bar.time,
                            bar_index: cand_global,
                        });
                    }
                }
            }

            self.pivot_highs.push_back(PivotRecord {
                rsi: cand_rsi,
                price: cand_bar.high,
                bar_index: cand_global,
                time: cand_bar.time,
            });
            if self.pivot_highs.len() > PIVOT_MEMORY {
                self.pivot_highs.pop_front();
            }
        }

        events
    }
}

// ---------------------------------------------------------------------------
// Redis snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsiDivSnapshot {
    pub timeframe: String,
    pub events: Vec<RsiDivEvent>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Live loop
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn rsi_div_loop(
    mut conn: MultiplexedConnection,
    http: Arc<reqwest::Client>,
    symbol: Arc<str>,
    seed_bars: Arc<Vec<Bar>>,
    timeframe: &'static str,
    candle_count: &'static str,
    redis_key: &'static str,
    interval_secs: u64,
) {
    let seed_cutoff = seed_bars.iter().map(|b| b.time).max();

    let mut seed_engine = RsiDivEngine::default_params()
        .with_rsi_filter(Some(60.0), Some(40.0));
    for bar in seed_bars.iter() {
        seed_engine.process_bar(bar.clone());
    }
    drop(seed_bars);

    info!("RsiDiv [{timeframe}]: seed warmup complete");

    let mut interval = time::interval(Duration::from_secs(interval_secs));
    loop {
        interval.tick().await;
        rsi_div_main(
            &mut conn,
            &http,
            &symbol,
            &seed_engine,
            seed_cutoff,
            timeframe,
            candle_count,
            redis_key,
            interval_secs,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn rsi_div_main(
    conn: &mut MultiplexedConnection,
    http: &reqwest::Client,
    symbol: &str,
    seed_engine: &RsiDivEngine,
    seed_cutoff: Option<DateTime<Utc>>,
    timeframe: &str,
    candle_count: &str,
    redis_key: &str,
    interval_secs: u64,
) {
    let candles = match fetch_bitget_candles(http, symbol, timeframe, candle_count).await {
        Ok(c) => c,
        Err(e) => {
            log::error!("RsiDiv [{timeframe}]: fetch error: {e}");
            return;
        }
    };

    let mut live: Vec<Bar> = candles
        .iter()
        .filter_map(|c| {
            let t = Utc.timestamp_millis_opt(c.timestamp).single()?;
            if seed_cutoff.is_none_or(|cut| t > cut) {
                Some(Bar {
                    time: t,
                    open: c.open,
                    high: c.high,
                    low: c.low,
                    close: c.close,
                    volume: Some(c.volume),
                    volume_quote: Some(c.quote_volume),
                })
            } else {
                None
            }
        })
        .collect();
    live.sort_by_key(|b| b.time);

    let mut engine = seed_engine.clone();
    let mut all_events: Vec<RsiDivEvent> = Vec::new();
    for bar in live {
        all_events.extend(engine.process_bar(bar));
    }

    let event_count = all_events.len();
    let snapshot = RsiDivSnapshot {
        timeframe: timeframe.to_string(),
        events: all_events,
        updated_at: Utc::now(),
    };

    let serialized = match serde_json::to_string(&snapshot) {
        Ok(s) => s,
        Err(e) => {
            log::error!("RsiDiv [{timeframe}]: serialisation error: {e}");
            return;
        }
    };

    let ttl = (interval_secs * 2) as usize;
    if let Err(e) = conn.set_ex::<_, _, ()>(redis_key, serialized, ttl).await {
        log::error!("RsiDiv [{timeframe}]: Redis write failed: {e}");
        return;
    }

    info!("RsiDiv [{timeframe}]: wrote {event_count} events to {redis_key}");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn bar(t_offset_secs: i64, o: f64, h: f64, l: f64, c: f64) -> Bar {
        Bar {
            time: Utc::now() + Duration::seconds(t_offset_secs),
            open: o,
            high: h,
            low: l,
            close: c,
            volume: None,
            volume_quote: None,
        }
    }

    /// Feed enough flat bars to warm up RSI (len=14 requires 15 bars with a
    /// previous close, but first bar has no prev — so 15 bars total before
    /// first RSI value is emitted).
    fn warmup_bars(engine: &mut RsiDivEngine, base_price: f64, count: usize) {
        for i in 0..count {
            engine.process_bar(bar(i as i64 * 60, base_price, base_price, base_price, base_price));
        }
    }

    #[test]
    fn test_regular_bullish_divergence() {
        // Regular Bullish: price makes Lower Low, RSI makes Higher Low
        // We craft two RSI pivot lows in range [5, 60]:
        //   pivot_1: low price = 90.0, RSI will be lower (price dropped hard)
        //   pivot_2: low price = 85.0 (lower), RSI will be higher (smaller drop)
        let mut eng = RsiDivEngine::new(14, 3, 3, 3, 60);

        // Warm up with neutral bars
        warmup_bars(&mut eng, 100.0, 20);

        let mut all_events: Vec<RsiDivEvent> = Vec::new();

        // First pivot low region: sharp drop → low RSI
        let sharp_drop = vec![
            bar(2000, 100.0, 100.0, 90.0, 90.0),
            bar(2060, 91.0, 91.0, 91.0, 91.0),
            bar(2120, 92.0, 92.0, 92.0, 92.0),
            bar(2180, 93.0, 93.0, 93.0, 93.0), // confirms pivot (lb_right=3 bars after)
            bar(2240, 94.0, 94.0, 94.0, 94.0),
            bar(2300, 95.0, 95.0, 95.0, 95.0),
        ];
        for b in sharp_drop {
            all_events.extend(eng.process_bar(b));
        }

        // Recovery
        for i in 0..10 {
            let p = 96.0 + i as f64 * 0.5;
            all_events.extend(eng.process_bar(bar(3000 + i * 60, p, p, p, p)));
        }

        // Second pivot low region: smaller drop → higher RSI, but lower absolute price
        let mild_drop = vec![
            bar(4000, 100.0, 100.0, 85.0, 86.0), // lower price, milder RSI drop
            bar(4060, 87.0, 87.0, 87.0, 87.0),
            bar(4120, 88.0, 88.0, 88.0, 88.0),
            bar(4180, 89.0, 89.0, 89.0, 89.0), // confirms pivot
            bar(4240, 90.0, 90.0, 90.0, 90.0),
            bar(4300, 91.0, 91.0, 91.0, 91.0),
        ];
        for b in mild_drop {
            all_events.extend(eng.process_bar(b));
        }

        let bull_count = all_events
            .iter()
            .filter(|e| matches!(e, RsiDivEvent::RegularBullish { .. }))
            .count();

        // At least one regular bullish divergence should have fired
        assert!(
            bull_count > 0,
            "expected RegularBullish divergence, got events: {all_events:?}"
        );
    }

    #[test]
    fn test_regular_bearish_divergence() {
        // Regular Bearish: price makes Higher High, RSI makes Lower High.
        //
        // Construction (verified analytically with Wilder's smoothing from RSI≈50):
        //   - Warmup: 30 alternating 100/102 bars → RSI≈50 (avg_gain≈avg_loss≈1)
        //   - Anchor bar at close=100 so the first rally starts cleanly
        //   - First strong rally: +4/bar × 4 → close=116, RSI≈70.3 at pivot
        //   - Mild decline: −2/bar × 3 → close=110, confirms first pivot high
        //   - Second weaker rally: +2/bar × 4 → close=118, RSI≈67.8 at pivot
        //     (higher price but lower RSI momentum → bearish divergence)
        //   - Decline: −2/bar × 3 → close=112, confirms second pivot high
        //
        // Expected: price high 119 > 117 (higher high), RSI 67.8 < 70.3 (lower high)
        let mut eng = RsiDivEngine::new(14, 3, 3, 5, 60);
        let mut t = 0i64;
        let mut all_events: Vec<RsiDivEvent> = Vec::new();

        // Warmup: 30 bars alternating 100/102 → RSI≈50
        for i in 0..30 {
            let c = if i % 2 == 0 { 100.0_f64 } else { 102.0_f64 };
            all_events.extend(eng.process_bar(bar(t, c, c + 1.0, c - 1.0, c)));
            t += 60;
        }
        // Anchor to close=100 so the rally gain arithmetic is exact
        all_events.extend(eng.process_bar(bar(t, 100.0, 101.0, 99.0, 100.0)));
        t += 60;

        // First strong rally: +4/bar × 4 (100→104→108→112→116)
        for close in [104.0_f64, 108.0, 112.0, 116.0] {
            all_events.extend(eng.process_bar(bar(t, close, close + 1.0, close - 1.0, close)));
            t += 60;
        }

        // Mild decline: −2/bar × 3 (116→114→112→110) — confirms first pivot high
        for close in [114.0_f64, 112.0, 110.0] {
            all_events.extend(eng.process_bar(bar(t, close, close + 1.0, close - 1.0, close)));
            t += 60;
        }

        // Second weaker rally: +2/bar × 4 (110→112→114→116→118)
        for close in [112.0_f64, 114.0, 116.0, 118.0] {
            all_events.extend(eng.process_bar(bar(t, close, close + 1.0, close - 1.0, close)));
            t += 60;
        }

        // Decline: −2/bar × 3 (118→116→114→112) — confirms second pivot high
        for close in [116.0_f64, 114.0, 112.0] {
            all_events.extend(eng.process_bar(bar(t, close, close + 1.0, close - 1.0, close)));
            t += 60;
        }

        let bear_count = all_events
            .iter()
            .filter(|e| matches!(e, RsiDivEvent::RegularBearish { .. }))
            .count();

        assert!(
            bear_count > 0,
            "expected RegularBearish divergence, got events: {all_events:?}"
        );
    }

    #[test]
    fn test_non_adjacent_divergence_recovered() {
        // Verifies the ring-buffer fix: Pivot A is kept alongside Pivot B so
        // that an A→C divergence is not lost when B (in-range from A, wrong
        // price/RSI direction) would previously have silently replaced A.
        //
        // Constructed scenario (lb_left=2, lb_right=2, range_lower=3, range_upper=50):
        //
        //   Pivot A (bar ~31): close=90, low=89, RSI≈36.4  — sharp drop from 100
        //   Pivot B (bar ~42): close=82, low=81, RSI≈31.0  — deeper drop; B.low < A.low
        //                                                     → no Regular Bullish A→B
        //                                                     (RSI not higher low)
        //   Pivot C (bar ~61): close=87, low=86, RSI≈39.0  — medium drop
        //     A→C: low 86 < 89 (LL ✓), RSI 39.0 > 36.4 (HL ✓) → RegularBullish ✓
        //     B→C: low 86 > 81 (not LL ✗)                      → no signal
        //
        // Old single-pivot code: B replaces A → C only sees B → signal missed.
        // New ring-buffer code:  [A, B] both live → C fires on A→C.
        let mut eng = RsiDivEngine::new(14, 2, 2, 3, 50);
        let mut t = 0i64;
        let mut all_events: Vec<RsiDivEvent> = Vec::new();

        let mut feed = |eng: &mut RsiDivEngine, close: f64| {
            let evs = eng.process_bar(bar(t, close, close + 1.0, close - 1.0, close));
            t += 60;
            evs
        };

        // Warmup: 30 alternating 100/102 + 1 anchor at 100 → RSI≈50
        for i in 0..30 {
            let c = if i % 2 == 0 { 100.0_f64 } else { 102.0_f64 };
            all_events.extend(feed(&mut eng, c));
        }
        all_events.extend(feed(&mut eng, 100.0)); // anchor

        // Pivot A: drop to 90, then +1 × 2 to confirm
        all_events.extend(feed(&mut eng, 90.0));
        all_events.extend(feed(&mut eng, 91.0));
        all_events.extend(feed(&mut eng, 92.0));

        // Recovery A→B: +1 × 8 back to ~100
        for c in [93.0, 94.0, 95.0, 96.0, 97.0, 98.0, 99.0, 100.0_f64] {
            all_events.extend(feed(&mut eng, c));
        }

        // Pivot B: drop to 82, then +1 × 2 to confirm (B.low < A.low, RSI even lower)
        all_events.extend(feed(&mut eng, 82.0));
        all_events.extend(feed(&mut eng, 83.0));
        all_events.extend(feed(&mut eng, 84.0));

        // Recovery B→C: +1 × 16 back to ~100
        for step in 0..16 {
            all_events.extend(feed(&mut eng, 85.0 + step as f64));
        }

        // Pivot C: drop to 87, then +1 × 2 to confirm
        // C.low=86 < A.low=89 (LL ✓) and C.rsi≈39 > A.rsi≈36.4 (HL ✓) → A→C divergence
        // C.low=86 > B.low=81 → no B→C divergence
        all_events.extend(feed(&mut eng, 87.0));
        all_events.extend(feed(&mut eng, 88.0));
        all_events.extend(feed(&mut eng, 89.0));

        let bull_count = all_events
            .iter()
            .filter(|e| matches!(e, RsiDivEvent::RegularBullish { .. }))
            .count();

        assert!(
            bull_count > 0,
            "expected A→C RegularBullish divergence via ring buffer, got events: {all_events:?}"
        );
    }

    #[test]
    fn test_rsi_zone_filter_suppresses_signal() {
        // Uses the same price sequence as test_regular_bearish_divergence, which
        // produces a RegularBearish with pivot RSI ≈ 67.8 (analytically verified).
        //
        // With bear_rsi_min = 70.0 the signal is suppressed (67.8 ≤ 70.0).
        // Without the filter the same sequence fires — confirming the filter is
        // what causes suppression, not a data issue.
        let price_sequence = |eng: &mut RsiDivEngine| -> Vec<RsiDivEvent> {
            let mut all: Vec<RsiDivEvent> = Vec::new();
            let mut t = 0i64;
            let mut feed = |e: &mut RsiDivEngine, c: f64| {
                let evs = e.process_bar(bar(t, c, c + 1.0, c - 1.0, c));
                t += 60;
                evs
            };
            for i in 0..30 {
                let c = if i % 2 == 0 { 100.0_f64 } else { 102.0_f64 };
                all.extend(feed(eng, c));
            }
            all.extend(feed(eng, 100.0));
            for c in [104.0_f64, 108.0, 112.0, 116.0] { all.extend(feed(eng, c)); }
            for c in [114.0_f64, 112.0, 110.0]         { all.extend(feed(eng, c)); }
            for c in [112.0_f64, 114.0, 116.0, 118.0]  { all.extend(feed(eng, c)); }
            for c in [116.0_f64, 114.0, 112.0]         { all.extend(feed(eng, c)); }
            all
        };

        // Without filter — signal fires (baseline check)
        let mut eng_no_filter = RsiDivEngine::new(14, 3, 3, 5, 60);
        let no_filter_events = price_sequence(&mut eng_no_filter);
        assert!(
            no_filter_events.iter().any(|e| matches!(e, RsiDivEvent::RegularBearish { .. })),
            "baseline: expected RegularBearish without filter"
        );

        // With bear_rsi_min = 70.0 — suppresses the ≈67.8 pivot RSI signal
        let mut eng_filtered = RsiDivEngine::new(14, 3, 3, 5, 60)
            .with_rsi_filter(None, Some(70.0));
        let filtered_events = price_sequence(&mut eng_filtered);
        let bear_count = filtered_events
            .iter()
            .filter(|e| matches!(e, RsiDivEvent::RegularBearish { .. }))
            .count();

        assert_eq!(
            bear_count, 0,
            "bear_rsi_min=70.0 should suppress RegularBearish when pivot RSI ≈ 67.8, got: {filtered_events:?}"
        );
    }

    #[test]
    fn test_hidden_bullish_divergence() {
        // Hidden Bullish: price Higher Low + RSI Lower Low → uptrend continuation.
        //
        // Key insight: RSI at a pivot low is determined by avg_loss history, not just
        // the single-bar price drop. A gentle multi-bar decline builds avg_loss slowly,
        // so a moderate price low can still have LOWER RSI than a sharper single-bar drop.
        //
        // Verified RSI arithmetic (Wilder, len=14, from warmup avg_gain≈0.929, avg_loss≈1.071):
        //
        //   Warmup anchor: 102→100 (loss 2) → avg_gain=0.929, avg_loss=1.071
        //
        //   Big rally +8/bar × 4 (100→132):
        //     After 4 bars: avg_gain≈2.749, avg_loss≈0.797
        //
        //   Drop -2/bar × 6 (132→120):
        //     After 6 bars: avg_gain≈1.761, avg_loss≈1.237
        //     RSI at close=120 ≈ 58.7  ← Pivot A (low=119)
        //
        //   Confirm +0.5/bar × 3 (120→121.5):
        //     RSI rises above 58.7, confirming Pivot A as local RSI minimum ✓
        //
        //   Gentle recovery +0.5/bar × 10 (121.5→126.5):
        //     avg_gain decays toward 0.5 steady-state; avg_gain≈0.991, avg_loss≈0.480
        //
        //   Drop -1/bar × 5 (126.5→121.5):
        //     After 5 bars: avg_gain≈0.684, avg_loss≈0.642
        //     RSI at close=121.5 ≈ 51.6  ← Pivot B (low=120.5)
        //
        //   low_B=120.5 > low_A=119  (Higher Low ✓)
        //   RSI_B=51.6  < RSI_A=58.7 (Lower RSI Low ✓)  → HiddenBullish
        //
        //   Confirm +0.5/bar × 3 → RSI rises above 51.6, confirming Pivot B ✓
        let mut eng = RsiDivEngine::new(14, 3, 3, 3, 60);
        let mut t = 0i64;
        let mut all_events: Vec<RsiDivEvent> = Vec::new();

        let mut feed = |eng: &mut RsiDivEngine, close: f64| {
            let evs = eng.process_bar(bar(t, close, close + 1.0, close - 1.0, close));
            t += 60;
            evs
        };

        // Warmup: alternating 100/102 + anchor → RSI ≈ 50
        for i in 0..30 {
            let c = if i % 2 == 0 { 100.0_f64 } else { 102.0_f64 };
            all_events.extend(feed(&mut eng, c));
        }
        all_events.extend(feed(&mut eng, 100.0)); // anchor (loss 2 from 102)

        // Big rally: avg_gain spikes, avg_loss decays
        for c in [108.0_f64, 116.0, 124.0, 132.0] {
            all_events.extend(feed(&mut eng, c));
        }

        // Pivot A: drop -2/bar × 6; center at close=120, RSI≈58.7, low=119
        for c in [130.0_f64, 128.0, 126.0, 124.0, 122.0, 120.0] {
            all_events.extend(feed(&mut eng, c));
        }

        // Confirm Pivot A: +0.5/bar × 3 (RSI rises above 58.7)
        for c in [120.5_f64, 121.0, 121.5] {
            all_events.extend(feed(&mut eng, c));
        }

        // Gentle recovery: avg_gain decays toward 0.5, avg_loss decays toward 0.5
        for step in 0..10 {
            all_events.extend(feed(&mut eng, 122.0 + step as f64 * 0.5));
        }

        // Pivot B: drop -1/bar × 5; center at close=121.5, RSI≈51.6, low=120.5
        for c in [125.5_f64, 124.5, 123.5, 122.5, 121.5] {
            all_events.extend(feed(&mut eng, c));
        }

        // Confirm Pivot B: +0.5/bar × 3 (RSI rises above 51.6)
        for c in [122.0_f64, 122.5, 123.0] {
            all_events.extend(feed(&mut eng, c));
        }

        let hidden_bull = all_events
            .iter()
            .filter(|e| matches!(e, RsiDivEvent::HiddenBullish { .. }))
            .count();

        assert!(
            hidden_bull > 0,
            "expected HiddenBullish divergence, got events: {all_events:?}"
        );
    }

    #[test]
    fn test_hidden_bearish_divergence() {
        // Hidden Bearish: price Lower High + RSI Higher High → downtrend continuation.
        //
        // Key insight: RSI at a pivot high is determined by avg_gain history. A gentle
        // multi-bar recovery rebuilds avg_gain from a depleted state, so a moderate
        // price high can still have HIGHER RSI than a stronger single-bar rally had.
        //
        // Verified RSI arithmetic (Wilder, len=14, from warmup avg_gain≈0.929, avg_loss≈1.071):
        //
        //   Crash -8/bar × 4 (100→68):
        //     After 4 bars: avg_gain≈0.692, avg_loss≈2.847
        //
        //   Rally +2/bar × 6 (68→80):
        //     After 6 bars: avg_gain≈1.166, avg_loss≈1.826
        //     RSI at close=80 ≈ 38.9  ← Pivot A (high=81)
        //
        //   Confirm -0.5/bar × 3 (80→78.5):
        //     RSI drops below 38.9, confirming Pivot A as local RSI maximum ✓
        //
        //   Gentle decline -0.5/bar × 10 (78.5→73.5):
        //     avg_loss decays toward 0.5 steady-state; avg_gain≈0.451, avg_loss≈1.013
        //
        //   Rally +1/bar × 5 (73.5→78.5):
        //     After 5 bars: avg_gain≈0.622, avg_loss≈0.701
        //     RSI at close=78.5 ≈ 47.0  ← Pivot B (high=79.5)
        //
        //   high_B=79.5 < high_A=81   (Lower High ✓)
        //   RSI_B=47.0  > RSI_A=38.9  (Higher RSI High ✓)  → HiddenBearish
        //
        //   Confirm -1/bar × 3 → RSI drops below 47.0, confirming Pivot B ✓
        let mut eng = RsiDivEngine::new(14, 3, 3, 3, 60);
        let mut t = 0i64;
        let mut all_events: Vec<RsiDivEvent> = Vec::new();

        let mut feed = |eng: &mut RsiDivEngine, close: f64| {
            let evs = eng.process_bar(bar(t, close, close + 1.0, close - 1.0, close));
            t += 60;
            evs
        };

        // Warmup: alternating 100/102 + anchor → RSI ≈ 50
        for i in 0..30 {
            let c = if i % 2 == 0 { 100.0_f64 } else { 102.0_f64 };
            all_events.extend(feed(&mut eng, c));
        }
        all_events.extend(feed(&mut eng, 100.0)); // anchor

        // Crash: avg_loss spikes, avg_gain decays
        for c in [92.0_f64, 84.0, 76.0, 68.0] {
            all_events.extend(feed(&mut eng, c));
        }

        // Pivot A: rally +2/bar × 6; center at close=80, RSI≈38.9, high=81
        for c in [70.0_f64, 72.0, 74.0, 76.0, 78.0, 80.0] {
            all_events.extend(feed(&mut eng, c));
        }

        // Confirm Pivot A: -0.5/bar × 3 (RSI drops below 38.9)
        for c in [79.5_f64, 79.0, 78.5] {
            all_events.extend(feed(&mut eng, c));
        }

        // Gentle decline: avg_loss decays toward 0.5, avg_gain decays toward 0
        for step in 0..10 {
            all_events.extend(feed(&mut eng, 78.0 - step as f64 * 0.5));
        }

        // Pivot B: rally +1/bar × 5; center at close=78.5, RSI≈47.0, high=79.5
        for c in [74.5_f64, 75.5, 76.5, 77.5, 78.5] {
            all_events.extend(feed(&mut eng, c));
        }

        // Confirm Pivot B: -1/bar × 3 (RSI drops below 47.0)
        for c in [77.5_f64, 76.5, 75.5] {
            all_events.extend(feed(&mut eng, c));
        }

        let hidden_bear = all_events
            .iter()
            .filter(|e| matches!(e, RsiDivEvent::HiddenBearish { .. }))
            .count();

        assert!(
            hidden_bear > 0,
            "expected HiddenBearish divergence, got events: {all_events:?}"
        );
    }

    #[test]
    fn test_out_of_range_pivot_ignored() {
        // Pivots more than `range_upper` bars apart should NOT trigger divergence
        let mut eng = RsiDivEngine::new(14, 3, 3, 5, 10); // tight range_upper = 10

        warmup_bars(&mut eng, 100.0, 20);

        let mut all_events: Vec<RsiDivEvent> = Vec::new();

        // First pivot low
        for b in [
            bar(100, 100.0, 100.0, 80.0, 82.0),
            bar(160, 83.0, 83.0, 83.0, 83.0),
            bar(220, 84.0, 84.0, 84.0, 84.0),
            bar(280, 85.0, 85.0, 85.0, 85.0),
        ] {
            all_events.extend(eng.process_bar(b));
        }

        // Feed 30 neutral bars (exceeds range_upper = 10)
        for i in 0..30 {
            all_events.extend(eng.process_bar(bar(1000 + i * 60, 100.0, 100.0, 100.0, 100.0)));
        }

        // Second pivot low (too far from first)
        for b in [
            bar(5000, 100.0, 100.0, 70.0, 72.0),
            bar(5060, 73.0, 73.0, 73.0, 73.0),
            bar(5120, 74.0, 74.0, 74.0, 74.0),
            bar(5180, 75.0, 75.0, 75.0, 75.0),
        ] {
            all_events.extend(eng.process_bar(b));
        }

        let div_count = all_events
            .iter()
            .filter(|e| matches!(e, RsiDivEvent::RegularBullish { .. } | RsiDivEvent::HiddenBullish { .. }))
            .count();

        assert_eq!(
            div_count, 0,
            "out-of-range pivots should not emit divergence, got: {all_events:?}"
        );
    }
}
