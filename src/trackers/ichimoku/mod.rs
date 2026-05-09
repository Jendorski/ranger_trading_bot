use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use log;
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::time;

use crate::exchange::bitget::{fetch_bitget_candles, Candle};
use crate::helper::{
    LAST_25_WEEKLY_ICHIMOKU_SPANS, TRADING_BOT_ICHIMOKU_CROSS, WEEKLY_CANDLES, WEEKLY_ICHIMOKU,
};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KumoCross {
    #[allow(dead_code)]
    Bullish,
    #[allow(dead_code)]
    Bearish,
}

#[derive(Debug, Serialize)]
pub struct Ichimoku {
    pub conversion_line: Vec<Option<f64>>, // Tenkan-sen
    pub base_line: Vec<Option<f64>>,       // Kijun-sen
    pub leading_span_a: Vec<Option<f64>>,  // Senkou A
    pub leading_span_b: Vec<Option<f64>>,  // Senkou B
    pub lagging_span: Vec<Option<f64>>,    // Chikou
}

/// Fetches live 1W candles from Bitget, computes the weekly Ichimoku, and writes
/// `TRADING_BOT_ICHIMOKU_CROSS`, `WEEKLY_ICHIMOKU`, `WEEKLY_CANDLES`, and
/// `LAST_25_WEEKLY_ICHIMOKU_SPANS` to Redis on every tick.
///
/// All errors are logged and the loop continues — Redis state is never left stale
/// by a silent failure.
pub async fn ichimoku_loop(
    mut conn: MultiplexedConnection,
    http: Arc<reqwest::Client>,
    symbol: Arc<str>,
    interval_secs: u64,
) {
    let mut interval = time::interval(Duration::from_secs(interval_secs));
    loop {
        interval.tick().await;

        let mut candles = match fetch_bitget_candles(&http, &symbol, "1W", "52").await {
            Ok(c) => c,
            Err(e) => {
                log::error!("IchimokuTracker: 1W candle fetch failed: {e}");
                continue;
            }
        };

        if candles.is_empty() {
            log::warn!("IchimokuTracker: 1W fetch returned no candles — skipping tick");
            continue;
        }

        candles.sort_by_key(|c| c.timestamp);

        let ichimoku = ichimoku_processor(&candles, 9, 26, 52, 26);

        if let Ok(s) = serde_json::to_string(&candles) {
            if let Err(e) = conn.set::<_, _, ()>(WEEKLY_CANDLES, s).await {
                log::error!("IchimokuTracker: Redis WEEKLY_CANDLES write failed: {e}");
            }
        }

        if let Ok(s) = serde_json::to_string(&ichimoku) {
            if let Err(e) = conn.set::<_, _, ()>(WEEKLY_ICHIMOKU, s).await {
                log::error!("IchimokuTracker: Redis WEEKLY_ICHIMOKU write failed: {e}");
            }
        }

        let (last_25_span_a, last_25_span_b) =
            get_last_25_spans(&ichimoku.leading_span_a, &ichimoku.leading_span_b);
        let mut l_25 = HashMap::new();
        l_25.insert("span_a", last_25_span_a);
        l_25.insert("span_b", last_25_span_b);
        if let Ok(s) = serde_json::to_string(&l_25) {
            if let Err(e) = conn.set::<_, _, ()>(LAST_25_WEEKLY_ICHIMOKU_SPANS, s).await {
                log::error!("IchimokuTracker: Redis LAST_25 write failed: {e}");
            }
        }

        match detect_kijun_spanb_state(&ichimoku) {
            Some(state) => {
                let snapshot = IchimokuCrossSnapshot {
                    state,
                    updated_at: Utc::now(),
                };
                match serde_json::to_string(&snapshot) {
                    Ok(s) => {
                        if let Err(e) =
                            conn.set::<_, _, ()>(TRADING_BOT_ICHIMOKU_CROSS, s).await
                        {
                            log::error!("IchimokuTracker: Redis ICHIMOKU_CROSS write failed: {e}");
                        } else {
                            log::info!(
                                "IchimokuTracker: {:?} written at {}",
                                state,
                                snapshot.updated_at
                            );
                        }
                    }
                    Err(e) => log::error!("IchimokuTracker: snapshot serialise failed: {e}"),
                }
            }
            None => {
                log::warn!(
                    "IchimokuTracker: could not detect Kijun/SpanB state — insufficient candles"
                );
            }
        }
    }
}

fn donchian_midpoint(candles: &[Candle], index: usize, length: usize) -> Option<f64> {
    if index + 1 < length {
        return None;
    }

    let start = index + 1 - length;
    let slice = &candles[start..=index];

    let lowest = slice.iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
    let highest = slice
        .iter()
        .map(|c| c.high)
        .fold(f64::NEG_INFINITY, f64::max);

    Some((lowest + highest) / 2.0)
}

fn ichimoku_processor(
    candles: &[Candle],
    conversion_periods: usize, // 9
    base_periods: usize,       // 26
    span_b_periods: usize,     // 52
    displacement: usize,       // 26
) -> Ichimoku {
    let len = candles.len();

    let mut conversion = vec![None; len];
    let mut base = vec![None; len];
    let mut span_a = vec![None; len + displacement];
    let mut span_b = vec![None; len + displacement];
    let mut lagging = vec![None; len];

    for i in 0..len {
        conversion[i] = donchian_midpoint(candles, i, conversion_periods);
        base[i] = donchian_midpoint(candles, i, base_periods);

        if i >= displacement {
            lagging[i - displacement] = Some(candles[i].close);
        }

        if let (Some(conv), Some(base)) = (conversion[i], base[i]) {
            span_a[i + displacement] = Some((conv + base) / 2.0);
        }

        if let Some(b) = donchian_midpoint(candles, i, span_b_periods) {
            span_b[i + displacement] = Some(b);
        }
    }

    Ichimoku {
        conversion_line: conversion,
        base_line: base,
        leading_span_a: span_a,
        leading_span_b: span_b,
        lagging_span: lagging,
    }
}

fn get_last_25_spans(
    span_a: &[Option<f64>],
    span_b: &[Option<f64>],
) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let start_a = span_a.len().saturating_sub(25);
    let start_b = span_b.len().saturating_sub(25);
    (span_a[start_a..].to_vec(), span_b[start_b..].to_vec())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IchimokuCrossState {
    KijunAboveSpanB,
    KijunBelowSpanB,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IchimokuCrossSnapshot {
    pub state: IchimokuCrossState,
    pub updated_at: DateTime<Utc>,
}

fn detect_kijun_spanb_state(ichimoku: &Ichimoku) -> Option<IchimokuCrossState> {
    let kijun = &ichimoku.base_line;
    let span_b = &ichimoku.leading_span_b;

    // Walk backward to find the most recent bar where both values are valid.
    // base_line[i] = Kijun at bar i.
    // leading_span_b[i] = SpanB projected to bar i (computed from candles[i-26]).
    for i in (0..kijun.len()).rev() {
        if let (Some(k), Some(s)) = (kijun[i], span_b[i]) {
            return Some(if k >= s {
                IchimokuCrossState::KijunAboveSpanB
            } else {
                IchimokuCrossState::KijunBelowSpanB
            });
        }
    }
    None
}

// ─── Ichimoku Baseline (Kijun-sen) ───────────────────────────────────────────

/// Streaming Kijun-sen: 26-period Donchian midpoint — (highest_high + lowest_low) / 2.
///
/// Feed bars in chronological order with [`IchimokuBaseline::update`].
/// [`IchimokuBaseline::value`] is `None` until the first 26 bars have been seen.
#[derive(Debug, Clone)]
pub struct IchimokuBaseline {
    highs: VecDeque<f64>,
    lows: VecDeque<f64>,
    pub value: Option<f64>,
}

impl IchimokuBaseline {
    const PERIOD: usize = 26;

    pub fn new() -> Self {
        Self {
            highs: VecDeque::with_capacity(Self::PERIOD),
            lows: VecDeque::with_capacity(Self::PERIOD),
            value: None,
        }
    }

    pub fn update(&mut self, high: f64, low: f64) {
        self.highs.push_back(high);
        self.lows.push_back(low);
        if self.highs.len() > Self::PERIOD {
            self.highs.pop_front();
            self.lows.pop_front();
        }
        if self.highs.len() == Self::PERIOD {
            let max_h = self.highs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let min_l = self.lows.iter().copied().fold(f64::INFINITY, f64::min);
            self.value = Some((max_h + min_l) / 2.0);
        }
    }
}

impl Default for IchimokuBaseline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_needs_26_bars() {
        let mut bl = IchimokuBaseline::new();
        for i in 0..25 {
            bl.update(100.0 + i as f64, 90.0 + i as f64);
            assert!(bl.value.is_none(), "should be None before 26 bars");
        }
        bl.update(125.0, 115.0);
        assert!(bl.value.is_some(), "should be Some after 26 bars");
    }

    #[test]
    fn baseline_midpoint_is_correct() {
        let mut bl = IchimokuBaseline::new();
        for _ in 0..26 {
            bl.update(200.0, 100.0);
        }
        assert!((bl.value.unwrap() - 150.0).abs() < 1e-9);
    }

    #[test]
    fn baseline_rolls_window() {
        let mut bl = IchimokuBaseline::new();
        for _ in 0..26 {
            bl.update(100.0, 50.0);
        }
        assert!((bl.value.unwrap() - 75.0).abs() < 1e-9);
        // Feed a bar with high=200, low=50 — new max_high = 200, baseline = 125
        bl.update(200.0, 50.0);
        assert!((bl.value.unwrap() - 125.0).abs() < 1e-9);
    }
}
