use std::time::Duration;

use log::info;
use redis::AsyncCommands;
use tokio::time;

use crate::bot::zones::{Side, Zone, Zones};
use crate::config::Config;
use crate::exchange::bitget::{self, Candle, CandleData, HttpCandleData};
use crate::helper::TRADING_BOT_ZONES;
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// OHLCV bar with timestamp
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bar {
    pub time: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: Option<f64>,
    pub volume_quote: Option<f64>,
}

/// Pivot type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PivotKind {
    High,
    Low,
}

/// Pivot data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pivot {
    pub kind: PivotKind,
    pub price: f64,
    pub time: DateTime<Utc>,
    pub index: usize,
}

/// Events emitted by the engine
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SMCEvent {
    PivotHigh {
        price: f64,
        time: DateTime<Utc>,
        index: usize,
    },
    PivotLow {
        price: f64,
        time: DateTime<Utc>,
        index: usize,
    },
    SweepHigh {
        price: f64,
        time: DateTime<Utc>,
        index: usize,
    }, // new pivot high > previous pivot high
    SweepLow {
        price: f64,
        time: DateTime<Utc>,
        index: usize,
    }, // new pivot low < previous pivot low
    BullishBOS {
        level: f64,
        time: DateTime<Utc>,
        index: usize,
    }, // price crossed up above pivot high (BOS)
    BearishBOS {
        level: f64,
        time: DateTime<Utc>,
        index: usize,
    }, // price crossed down below pivot low
    StrongLow {
        price: f64,
        time: DateTime<Utc>,
        index: usize,
    }, // Sweep low followed by bullish BOS (LONG)
    StrongHigh {
        price: f64,
        time: DateTime<Utc>,
        index: usize,
    }, // Sweep high followed by bearish BOS (SHORT)
}

/// The main engine. Use `process_bar` for each new bar (in chronological order).
#[derive(Debug, Clone)]
pub struct SmcEngine {
    /// lookback used to detect local pivot: pivot when value is extreme compared to `left` previous bars and `right` future bars
    pivot_left: usize,
    pivot_right: usize,
    /// Bars buffer (needed because pivot detection checks "future" right bars)
    bars: Vec<Bar>,
    /// Last pivot high & low
    last_pivot_high: Option<Pivot>,
    last_pivot_low: Option<Pivot>,
    /// Most recent sweep indicators: preserve the pivot (sweep) until a BOS occurs
    pending_sweep_low: Option<Pivot>,
    pending_sweep_high: Option<Pivot>,
    /// Keep last known BOS levels (to avoid double emitting)
    last_bullish_bos_level: Option<f64>,
    last_bearish_bos_level: Option<f64>,
}

impl SmcEngine {
    /// Create engine; pivot_left+pivot_right define pivot detection window (e.g., 3 & 3)
    pub fn new(pivot_left: usize, pivot_right: usize) -> Self {
        Self {
            pivot_left,
            pivot_right,
            bars: Vec::new(),
            last_pivot_high: None,
            last_pivot_low: None,
            pending_sweep_low: None,
            pending_sweep_high: None,
            last_bullish_bos_level: None,
            last_bearish_bos_level: None,
        }
    }

    /// Process a new bar (in chronological order). Returns events that occurred at this bar.
    ///
    /// Note: Because pivot detection needs `pivot_right` future bars, a pivot emitted for
    /// index i will only be discovered once we've added bars up to i + pivot_right.
    pub fn process_bar(&mut self, bar: Bar) -> Vec<SMCEvent> {
        self.bars.push(bar);
        let idx = self.bars.len() - 1;
        let mut events = Vec::new();

        // can't detect pivot until we have pivot_left past bars and pivot_right future bars
        if idx < self.pivot_left + self.pivot_right {
            return events;
        }

        // candidate pivot index is the current index - pivot_right (centered pivot)
        let cand_idx = idx - self.pivot_right;
        // find whether cand_idx is pivot high or pivot low
        let is_pivot_high = {
            let cand_high = self.bars[cand_idx].high;
            let mut ok = true;
            // compare with left and right bars highs
            let left_start = cand_idx.saturating_sub(self.pivot_left);
            for i in left_start..cand_idx {
                if self.bars[i].high >= cand_high {
                    ok = false;
                    break;
                }
            }
            if ok {
                for i in (cand_idx + 1)..=(cand_idx + self.pivot_right) {
                    if self.bars[i].high >= cand_high {
                        ok = false;
                        break;
                    }
                }
            }
            ok
        };

        let is_pivot_low = {
            let cand_low = self.bars[cand_idx].low;
            let mut ok = true;
            let left_start = cand_idx.saturating_sub(self.pivot_left);
            for i in left_start..cand_idx {
                if self.bars[i].low <= cand_low {
                    ok = false;
                    break;
                }
            }
            if ok {
                for i in (cand_idx + 1)..=(cand_idx + self.pivot_right) {
                    if self.bars[i].low <= cand_low {
                        ok = false;
                        break;
                    }
                }
            }
            ok
        };

        // Emit pivot events & handle sweep logic
        if is_pivot_low {
            let p = Pivot {
                kind: PivotKind::Low,
                price: self.bars[cand_idx].low,
                time: self.bars[cand_idx].time,
                index: cand_idx,
            };
            events.push(SMCEvent::PivotLow {
                price: p.price,
                time: p.time,
                index: p.index,
            });

            // sweep detection: if this pivot low is lower than previous pivot low => sweep
            if let Some(prev_low) = &self.last_pivot_low {
                if p.price < prev_low.price {
                    // mark pending sweep low
                    self.pending_sweep_low = Some(p.clone());
                    events.push(SMCEvent::SweepLow {
                        price: p.price,
                        time: p.time,
                        index: p.index,
                    });
                }
            } else {
                // first pivot low seen -> not a sweep yet but store
            }
            self.last_pivot_low = Some(p);
        }

        if is_pivot_high {
            let p = Pivot {
                kind: PivotKind::High,
                price: self.bars[cand_idx].high,
                time: self.bars[cand_idx].time,
                index: cand_idx,
            };
            events.push(SMCEvent::PivotHigh {
                price: p.price,
                time: p.time,
                index: p.index,
            });

            if let Some(prev_high) = &self.last_pivot_high {
                if p.price > prev_high.price {
                    self.pending_sweep_high = Some(p.clone());
                    events.push(SMCEvent::SweepHigh {
                        price: p.price,
                        time: p.time,
                        index: p.index,
                    });
                }
            }
            self.last_pivot_high = Some(p);
        }

        // Structure break detection (BOS)
        // Bullish BOS: current close crosses above the most recent pivot high
        if let Some(p_high) = &self.last_pivot_high {
            let close = self.bars[idx].close;
            // avoid repeated BOS emits for the same level
            let crossed_up = close > p_high.price
                && (self.last_bullish_bos_level.is_none()
                    || (self.last_bullish_bos_level.unwrap() != p_high.price));
            if crossed_up {
                events.push(SMCEvent::BullishBOS {
                    level: p_high.price,
                    time: self.bars[idx].time,
                    index: idx,
                });
                self.last_bullish_bos_level = Some(p_high.price);

                // If there was a pending sweep low (sweep happened before this BOS), emit StrongLow
                if let Some(sweep_low) = &self.pending_sweep_low {
                    // ensure the sweep happened before current BOS and sweep refers to a low prior to the high (basic sanity)
                    if sweep_low.index < p_high.index {
                        events.push(SMCEvent::StrongLow {
                            price: sweep_low.price,
                            time: self.bars[idx].time,
                            index: idx,
                        });
                        // clear pending sweep low after it is used
                        self.pending_sweep_low = None;
                    } else {
                        // if sweep low occurred after the pivot high, still consider (depends on desired policy)
                        events.push(SMCEvent::StrongLow {
                            price: sweep_low.price,
                            time: self.bars[idx].time,
                            index: idx,
                        });
                        self.pending_sweep_low = None;
                    }
                }
            }
        }

        // Bearish BOS: close crosses below most recent pivot low
        if let Some(p_low) = &self.last_pivot_low {
            let close = self.bars[idx].close;
            let crossed_down = close < p_low.price
                && (self.last_bearish_bos_level.is_none()
                    || (self.last_bearish_bos_level.unwrap() != p_low.price));
            if crossed_down {
                events.push(SMCEvent::BearishBOS {
                    level: p_low.price,
                    time: self.bars[idx].time,
                    index: idx,
                });
                self.last_bearish_bos_level = Some(p_low.price);

                if let Some(sweep_high) = &self.pending_sweep_high {
                    events.push(SMCEvent::StrongHigh {
                        price: sweep_high.price,
                        time: self.bars[idx].time,
                        index: idx,
                    });
                    self.pending_sweep_high = None;
                }
            }
        }

        // Return events for this bar (possibly empty)
        events
    }
}

///15m, 333
/// 4H, 1000
/// TODO, make configurable the time frame and the number of candles
async fn return_data(timeframe: String, limit: String) -> Vec<Bar> {
    let bitget_candles = <HttpCandleData as bitget::CandleData>::new();
    let res: Result<Vec<Candle>, anyhow::Error> =
        bitget_candles.get_bitget_candles(timeframe, limit).await;
    let candle_data = res.unwrap_or_else(|_| Vec::new());
    if candle_data.is_empty() {
        return Vec::new();
    }
    let mut bars: Vec<Bar> = Vec::new();
    for candle in candle_data {
        bars.push(Bar {
            time: Utc.timestamp_millis_opt(candle.timestamp).unwrap(),
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            volume: Some(candle.volume),
            volume_quote: Some(candle.quote_volume),
        });
    }
    bars
}

//A customizable loop that will run at configured times
// If we need 4H candle data, we can run the loop every 30minutes so we can be on-sync with the changes as the market can move fast
//If we need 15m candle data, we can run the loop every 45 seconds so we can be on-sync with the changes as the market can move fast
pub async fn smc_loop(mut conn: redis::aio::MultiplexedConnection, config: Config) {
    let mut interval = time::interval(Duration::from_secs(config.smc_loop_interval));

    loop {
        interval.tick().await;
        smc_main(&mut conn, &config).await;
    }
}

fn remove_conflicting_zones(
    sweep_highs: Vec<Zone>,
    sweep_lows: Vec<Zone>,
    min_distance: f64,
) -> (Vec<Zone>, Vec<Zone>) {
    let mut keep_highs = vec![true; sweep_highs.len()];
    let mut keep_lows = vec![true; sweep_lows.len()];

    for (i, high) in sweep_highs.iter().enumerate() {
        for (j, low) in sweep_lows.iter().enumerate() {
            let distance = high.low - low.high;

            if distance < min_distance && distance > 0.0 {
                // Remove the weaker zone (you can use other criteria)
                // Here we remove based on zone width
                let high_width = high.high - high.low;
                let low_width = low.high - low.low;

                if high_width < low_width {
                    keep_highs[i] = false;
                } else {
                    keep_lows[j] = false;
                }
            }
        }
    }

    let filtered_highs: Vec<Zone> = sweep_highs
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep_highs[*i])
        .map(|(_, z)| z)
        .collect();

    let filtered_lows: Vec<Zone> = sweep_lows
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep_lows[*i])
        .map(|(_, z)| z)
        .collect();

    (filtered_highs, filtered_lows)
}

fn filter_close_zones(mut zones: Vec<Zone>, min_distance: f64) -> Vec<Zone> {
    if zones.is_empty() {
        return zones;
    }

    // Sort by midpoint
    zones.sort_by(|a, b| a.midpoint().partial_cmp(&b.midpoint()).unwrap());

    let mut filtered = vec![zones[0]];

    for zone in zones.into_iter().skip(1) {
        let last_accepted = filtered.last().unwrap();

        if !zone.overlaps_or_too_close(last_accepted, min_distance) {
            filtered.push(zone);
        }
    }

    filtered
}

// Convert the candles to Bar, which are used to find the Strong Lows and Strong Highs, then convert the Bar to Zones needed for trading.
///todo!: setup config for the pivot low and pivot high
async fn smc_main(conn: &mut redis::aio::MultiplexedConnection, config: &Config) {
    let mut eng = SmcEngine::new(3, 3);
    let mut sample_bars = return_data(
        config.smc_timeframe.clone(),
        config.smc_candle_count.clone(),
    )
    .await;

    sample_bars.sort_by_key(|s| s.time);

    let mut sweep_lows: Vec<Zone> = Vec::new();
    let mut sweep_highs: Vec<Zone> = Vec::new();

    for b in sample_bars {
        let events = eng.process_bar(b);
        for ev in events {
            //println!("{}", serde_json::to_string(&ev).unwrap());
            match ev {
                SMCEvent::StrongLow {
                    price,
                    time: _,
                    index: _,
                } => {
                    //sweep_lows.push(SMCEvent::StrongLow { price, time, index });
                    let low_low = price - (price * config.smc_zone_multiplier);
                    sweep_lows.push(Zone {
                        low: low_low,
                        high: price,
                        side: Side::Long,
                    });
                }
                SMCEvent::StrongHigh {
                    price,
                    time: _,
                    index: _,
                } => {
                    //sweep_highs.push(SMCEvent::StrongHigh { price, time, index });
                    let high_high = price + (price * config.smc_zone_multiplier);
                    sweep_highs.push(Zone {
                        low: price,
                        high: high_high,
                        side: Side::Short,
                    });
                }
                _ => {}
            }
        }
    }

    let (filtered_highs, filtered_lows) =
        remove_conflicting_zones(sweep_highs, sweep_lows, config.smc_min_distance);

    let long_zones = filter_close_zones(filtered_lows, config.smc_min_distance);
    let short_zones = filter_close_zones(filtered_highs, config.smc_min_distance);

    if short_zones.is_empty() || long_zones.is_empty() {
        info!("No zones found");
        return;
    }

    let zones = Zones {
        long_zones,
        short_zones,
    };

    info!("zones.long_zones: {:?}", zones.long_zones);
    info!("zones.short_zones: {:?}", zones.short_zones);

    // Save the zones to redis
    let serialized_zones = serde_json::to_string(&zones).unwrap();
    let _: () = conn.set(TRADING_BOT_ZONES, serialized_zones).await.unwrap();
}

// -------------------------- Example usage --------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_bar(t: DateTime<Utc>, o: f64, h: f64, l: f64, c: f64) -> Bar {
        Bar {
            time: t,
            open: o,
            high: h,
            low: l,
            close: c,
            volume: None,
            volume_quote: None,
        }
    }

    #[test]
    fn test_strong_low_detection() {
        // small example with artificial bars to create: pivot low sweep then bullish BOS
        let mut eng = SmcEngine::new(2, 2);
        let start = Utc::now();

        // Need enough bars to establish:
        // 1. Pivot Low 1
        // 2. Pivot High 1
        // 3. Pivot Low 2 (Sweep Low)
        // 4. Bullish BOS (Close > Pivot High 1)
        let bars = vec![
            make_bar(start + Duration::seconds(0), 100.0, 100.0, 100.0, 100.0), // 0
            make_bar(start + Duration::seconds(60), 101.0, 101.0, 101.0, 101.0), // 1
            make_bar(start + Duration::seconds(120), 95.0, 95.0, 95.0, 95.0),   // 2: Pivot Low 1
            make_bar(start + Duration::seconds(180), 101.0, 101.0, 101.0, 101.0), // 3
            make_bar(start + Duration::seconds(240), 100.0, 100.0, 100.0, 100.0), // 4 -> ID Pivot Low 1
            make_bar(start + Duration::seconds(300), 110.0, 110.0, 110.0, 110.0), // 5: Pivot High 1
            make_bar(start + Duration::seconds(360), 100.0, 100.0, 100.0, 100.0), // 6
            make_bar(start + Duration::seconds(420), 101.0, 101.0, 101.0, 101.0), // 7 -> ID Pivot High 1
            make_bar(start + Duration::seconds(480), 90.0, 90.0, 90.0, 90.0), // 8: Pivot Low 2 (Sweep!!)
            make_bar(start + Duration::seconds(540), 100.0, 100.0, 100.0, 100.0), // 9
            make_bar(start + Duration::seconds(600), 105.0, 105.0, 105.0, 105.0), // 10 -> ID Pivot Low 2
            make_bar(start + Duration::seconds(660), 115.0, 115.0, 115.0, 115.0), // 11 -> Bullish BOS -> Strong Low!
        ];

        let mut emitted = Vec::new();
        for b in bars {
            let evs = eng.process_bar(b);
            for e in evs {
                // serialize for readable assert/debug
                let js = serde_json::to_string(&e).unwrap();
                emitted.push(js);
            }
        }

        // There should be at least one StrongLow event in emitted array
        let found_strong_low = emitted.iter().any(|s| s.contains("\"StrongLow\""));
        assert!(
            found_strong_low,
            "expected StrongLow in events, got {emitted:?}"
        );
    }
}
