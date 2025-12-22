use std::sync::Arc;
use std::time::Duration;

use log::info;
use redis::AsyncCommands;
use tokio::time;

use crate::bot::{Zone, Zones};
use crate::config::Config;
use crate::exchange::bitget::{self, Candle, CandleData, HttpCandleData};
use crate::exchange::{Exchange, HttpExchange};
use crate::helper::{TRADING_BOT_SMART_MONEY_CONCEPTS_NEXT_CALL, TRADING_BOT_ZONES};
use chrono::TimeZone;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_cron_scheduler::{Job, JobScheduler};

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
                    if sweep_high.index < p_low.index {
                        events.push(SMCEvent::StrongHigh {
                            price: sweep_high.price,
                            time: self.bars[idx].time,
                            index: idx,
                        });
                        self.pending_sweep_high = None;
                    } else {
                        events.push(SMCEvent::StrongHigh {
                            price: sweep_high.price,
                            time: self.bars[idx].time,
                            index: idx,
                        });
                        self.pending_sweep_high = None;
                    }
                }
            }
        }

        // Return events for this bar (possibly empty)
        events
    }

    async fn smc_process_pre_new_target(&mut self, conn: &mut redis::aio::MultiplexedConnection) {
        let config = Config::from_env().unwrap();
        let smc_time_frame = config.smc_timeframe;
        let smc_candle_count = config.smc_candle_count;

        let mut sample_bars = return_data(smc_time_frame, smc_candle_count).await;

        sample_bars.sort_by_key(|s| s.time);

        let last = sample_bars.last().unwrap();
        info!("last.time: {:?}", last.time.timestamp());
        let next = (last.time + Duration::from_secs(60 * 60 * 4)).timestamp();
        info!("next: {}", next);

        let diff_seconds = next - Utc::now().timestamp();
        info!("diff_seconds: {}", diff_seconds);

        //it is the diff_seconds we would use to make the next call, get the latest close
        let _: () = conn
            .set(
                TRADING_BOT_SMART_MONEY_CONCEPTS_NEXT_CALL,
                diff_seconds.to_string(),
            )
            .await
            .unwrap();

        let sched = JobScheduler::new().await.unwrap();

        let conn_clone = conn.clone();
        let job = Job::new_one_shot_async(
            Duration::from_secs(diff_seconds.try_into().unwrap_or(14400)), // 4 hours
            move |_uuid, _l| {
                let mut conn = conn_clone.clone();
                Box::pin(async move {
                    Self::smc_next_call(&mut conn).await;
                })
            },
        )
        .unwrap();
        sched.add(job).await.unwrap();
        tokio::spawn(async move { sched.start().await });
    }

    pub async fn smc_find_targets(
        mut self,
        conn: &mut redis::aio::MultiplexedConnection,
        price: f64,
    ) {
        let cached_zones: String = conn
            .get(TRADING_BOT_ZONES)
            .await
            .unwrap_or(String::from("[]"));

        let zones: Zones = serde_json::from_str(&cached_zones).unwrap_or(Zones {
            long_zones: vec![],
            short_zones: vec![],
        });

        if zones.long_zones.is_empty() || zones.short_zones.is_empty() {
            info!("No zones found");
            return;
        }

        let long_zone = zones.long_zones[0];
        let short_zone = zones.short_zones[0];

        if price < long_zone.low {
            info!("Price is below the long zone low!");
            Self::smc_process_pre_new_target(&mut self, conn).await;
        }

        if price > short_zone.high {
            info!("Price is above the short zone high");
            Self::smc_process_pre_new_target(&mut self, conn).await;
        }
    }

    pub async fn smc_next_call(conn: &mut redis::aio::MultiplexedConnection) -> () {
        let config = Config::from_env().unwrap();
        let smc_time_frame = config.smc_timeframe;
        let smc_candle_count = config.smc_candle_count;

        //Get the current zones
        let cached_zones: String = conn.get(TRADING_BOT_ZONES).await.unwrap();
        let zones: Zones = serde_json::from_str(&cached_zones).unwrap();
        let long_zone = zones.long_zones[0];
        info!("long_zone: {:?}", long_zone);

        let short_zone = zones.short_zones[0];
        info!("short_zone: {:?}", short_zone);

        //Get the price from the exchange API
        let exchange = Arc::new(HttpExchange {
            client: reqwest::Client::new(),
            symbol: Config::from_env().unwrap().symbol,
        });
        let price = exchange.get_current_price().await.unwrap();
        info!("price: {:?}", price);

        //Get the latest close from the exchange API
        let bar_data = return_data(smc_time_frame, smc_candle_count).await;
        let last = bar_data.last().unwrap();
        info!("last.close: {}", last.close);

        let diff_between_past_zones = short_zone.high - long_zone.low;
        info!("diff_between_past_zones: {:?}", diff_between_past_zones);

        //let's assume the price is 82,000 and the latest close is 82,741
        //And our Zones are:
        //short_zone => low: 94543.6, high: 94614.5077
        //long_zone => low: 83714.866725, high: 83777.7
        //If the price is below the low of the LONG ZONE and price is below the latest close, take the difference between zones and use it to construct another Zone and open a SHORT position
        //If the price is above the high of the SHORT ZONE and price is above the latest close, take the difference between zones and use it to construct another Zone and open a LONG position
        if price > short_zone.high && price > last.close {
            info!(
                "Price is above the last close!. Price is also above the latest close, 
            This means it's time to execute a new LONG and create a new ZONE"
            );
            let new_short_zone_high = short_zone.high + diff_between_past_zones;
            let new_short_zone_low_diff = new_short_zone_high * 0.00075;
            let new_short_zone_low = new_short_zone_high - new_short_zone_low_diff;
            let new_short_zone = Zone {
                low: new_short_zone_low,
                high: new_short_zone_high,
            };
            info!("new_short_zone: {:?}", new_short_zone);

            let new_long_zone_high = price;
            let new_long_zone_low_diff = new_long_zone_high * 0.00075;
            let new_long_zone_low = new_long_zone_high - new_long_zone_low_diff;
            let new_long_zone = Zone {
                low: new_long_zone_low,
                high: new_long_zone_high,
            };
            info!("new_long_zone: {:?}", new_long_zone);

            let mut new_zones = zones.clone();
            new_zones.long_zones.push(new_long_zone);
            new_zones.short_zones.push(new_short_zone);
            info!("new_zones: {:?}", new_zones);
            // let serialized = serde_json::to_string(&new_zones).unwrap();
            // let _: () = conn.set(TRADING_BOT_ZONES, serialized).await.unwrap();
        }

        if price < long_zone.low && price < last.close {
            info!(
                "Price is below the last close!. Price is also below the latest close, 
                This means it's time to execute a SHORT and create a new ZONE"
            );

            //Take the difference between the previous zones and use it to construct another Zone
            let new_long_zone_high = long_zone.low - diff_between_past_zones;
            info!("new_long_zone_high: {:?}", new_long_zone_high);

            let new_long_zone_low_diff = new_long_zone_high * 0.00075;
            info!("new_long_zone_low_diff: {:?}", new_long_zone_low_diff);

            let new_long_zone_low = new_long_zone_high - new_long_zone_low_diff;
            info!("new_long_zone_low: {:?}", new_long_zone_low);

            let new_long_zone = Zone {
                low: new_long_zone_low,
                high: new_long_zone_high,
            };
            info!("new_long_zone: {:?}", new_long_zone);

            let new_short_zone_high = price;
            let short_zone_low_diff = new_short_zone_high * 0.00075;
            info!("short_zone_low_diff: {:?}", short_zone_low_diff);
            let new_short_zone_low = new_short_zone_high - short_zone_low_diff;
            info!("new_short_zone_low: {:?}", new_short_zone_low);

            let new_short_zone = Zone {
                low: new_short_zone_low,
                high: new_short_zone_high,
            };
            info!("new_short_zone: {:?}", new_short_zone);

            let mut new_zones = zones.clone();
            new_zones.long_zones.push(new_long_zone);
            new_zones.short_zones.push(new_short_zone);
            info!("new_zones: {:?}", new_zones);
            //let serialized = serde_json::to_string(&zones).unwrap();
            //let _: () = conn.set(TRADING_BOT_ZONES, serialized).await.unwrap();
        }

        let _: () = conn
            .del(TRADING_BOT_SMART_MONEY_CONCEPTS_NEXT_CALL)
            .await
            .unwrap();
        ()
    }
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

        // create upward swing -> set a pivot high at index ~2
        let bars = vec![
            make_bar(start + Duration::seconds(0), 100.0, 101.0, 99.5, 100.5),
            make_bar(start + Duration::seconds(60), 100.5, 102.0, 100.0, 101.5),
            make_bar(start + Duration::seconds(120), 101.5, 103.0, 101.0, 102.5), // pivot high candidate
            // drop and create sweep low (new pivot low lower than previous lows)
            make_bar(start + Duration::seconds(180), 102.5, 103.0, 98.0, 98.5),
            make_bar(start + Duration::seconds(240), 98.5, 99.0, 97.5, 98.0), // pivot low candidate (sweep)
            // price recovers and crosses above the last pivot high -> bullish BOS -> StrongLow emitted
            make_bar(start + Duration::seconds(300), 98.0, 104.0, 97.5, 103.5),
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
            "expected StrongLow in events, got {:?}",
            emitted
        );
    }
}

///15m, 333
/// 4H, 1000
/// TODO, make configurable the time frame and the number of candles
///15m, 333
/// 4H, 1000
/// TODO, make configurable the time frame and the number of candles
async fn return_data(timeframe: String, limit: String) -> Vec<Bar> {
    let bitget_candles = <HttpCandleData as bitget::CandleData>::new();
    let res: Result<Vec<Candle>, anyhow::Error> =
        bitget_candles.get_bitget_candles(timeframe, limit).await;
    let candle_data = res.unwrap_or_else(|_| Vec::new());
    if candle_data.len() == 0 {
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
    let loop_interval_seconds = 180; //1800 == 30mins, 45==45seconds

    let mut interval = time::interval(Duration::from_secs(loop_interval_seconds));

    loop {
        interval.tick().await;
        smc_main(
            &mut conn,
            config.smc_timeframe.clone(),
            config.smc_candle_count.clone(),
        )
        .await;
    }
}

// Convert the candles to Bar, which are used to find the Strong Lows and Strong Highs, then convert the Bar to Zones needed for trading.
///todo!: setup config for the pivot low and pivot high
async fn smc_main(conn: &mut redis::aio::MultiplexedConnection, timeframe: String, limit: String) {
    let mut eng = SmcEngine::new(3, 3);
    let mut sample_bars = return_data(timeframe, limit).await;

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
                    let low_low = price - (price * 0.00075);
                    sweep_lows.push(Zone {
                        low: low_low,
                        high: price,
                    });
                }
                SMCEvent::StrongHigh {
                    price,
                    time: _,
                    index: _,
                } => {
                    //sweep_highs.push(SMCEvent::StrongHigh { price, time, index });
                    let high_high = price + (price * 0.00075);
                    sweep_highs.push(Zone {
                        low: price,
                        high: high_high,
                    });
                }
                _ => {}
            }
        }
    }

    let long_zone = sweep_lows
        .iter()
        .min_by(|a, b| a.low.partial_cmp(&b.low).unwrap())
        .cloned()
        .unwrap_or(Zone {
            low: 0.0,
            high: 0.0,
        });

    if long_zone.low == 0.0 || long_zone.high == 0.0 {
        info!("No long zone found");
        return;
    }

    // For short zones, find the zone with the highest price (maximum high value)
    let short_zone = sweep_highs
        .iter()
        .max_by(|a, b| a.high.partial_cmp(&b.high).unwrap())
        .cloned()
        .unwrap_or(Zone {
            low: 0.0,
            high: 0.0,
        });

    if short_zone.low == 0.0 || short_zone.high == 0.0 {
        info!("No short zone found");
        return;
    }

    let zones = Zones {
        long_zones: [long_zone].to_vec(),
        short_zones: [short_zone].to_vec(),
    };

    info!("zones.long_zones: {:?}", zones.long_zones);
    info!("zones.short_zones: {:?}", zones.short_zones);

    // Save the zones to redis
    let serialized_zones = serde_json::to_string(&zones).unwrap();
    let _: () = conn.set(TRADING_BOT_ZONES, serialized_zones).await.unwrap();

    ()
}
