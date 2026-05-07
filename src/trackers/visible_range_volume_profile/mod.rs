use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone;
use chrono::Utc;
use log::info;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::time;

use crate::exchange::bitget::fetch_bitget_candles;
use crate::helper::TRADING_BOT_VRVP;
use crate::trackers::smart_money_concepts::Bar;

// ─── Core data types ──────────────────────────────────────────────────────────

/// A single price bucket in the volume profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrvpBin {
    pub price_low: f64,
    pub price_high: f64,
    /// Midpoint — convenient reference price for this bucket.
    pub price_mid: f64,
    /// Total volume allocated to this bucket.
    pub volume: f64,
}

/// Classification of a price level relative to the overall volume distribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeType {
    /// Volume significantly above mean — price tends to consolidate here.
    HighVolumeNode,
    /// Volume significantly below mean — price tends to move through quickly.
    LowVolumeNode,
    /// Volume near the mean.
    Neutral,
}

/// A bin annotated with its HVN / LVN classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrvpNode {
    pub bin: VrvpBin,
    pub node_type: NodeType,
}

/// Full VRVP result for the bars provided.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrvpProfile {
    /// All bins, sorted low → high by price.
    pub bins: Vec<VrvpBin>,
    /// Every bin annotated as HVN, LVN, or Neutral.
    pub nodes: Vec<VrvpNode>,
    /// Point of Control — midpoint of the highest-volume bin.
    pub poc: f64,
    /// Value Area High — upper boundary of the 70 % value area.
    pub vah: f64,
    /// Value Area Low — lower boundary of the 70 % value area.
    pub val: f64,
    /// Sum of all bar volumes.
    pub total_volume: f64,
    /// Volume captured inside the value area.
    pub value_area_volume: f64,
}

// ─── Profile query API ────────────────────────────────────────────────────────

#[allow(dead_code)]
impl VrvpProfile {
    /// Returns the [`NodeType`] of whichever bin contains `price`.
    /// Returns [`NodeType::Neutral`] if price is outside the profile range.
    pub fn node_at(&self, price: f64) -> NodeType {
        self.nodes
            .iter()
            .find(|n| price >= n.bin.price_low && price < n.bin.price_high)
            .map(|n| n.node_type.clone())
            .unwrap_or(NodeType::Neutral)
    }

    /// Returns the midpoint of the nearest HVN strictly above `price` (`bullish = true`)
    /// or strictly below `price` (`bullish = false`).
    pub fn nearest_hvn_in_direction(&self, price: f64, bullish: bool) -> Option<f64> {
        if bullish {
            self.nodes
                .iter()
                .filter(|n| n.node_type == NodeType::HighVolumeNode && n.bin.price_low > price)
                .min_by(|a, b| a.bin.price_mid.partial_cmp(&b.bin.price_mid).unwrap())
                .map(|n| n.bin.price_mid)
        } else {
            self.nodes
                .iter()
                .filter(|n| n.node_type == NodeType::HighVolumeNode && n.bin.price_high < price)
                .max_by(|a, b| a.bin.price_mid.partial_cmp(&b.bin.price_mid).unwrap())
                .map(|n| n.bin.price_mid)
        }
    }

    /// Returns the midpoint of the nearest LVN strictly above `price` (`bullish = true`)
    /// or strictly below `price` (`bullish = false`).
    pub fn nearest_lvn_in_direction(&self, price: f64, bullish: bool) -> Option<f64> {
        if bullish {
            self.nodes
                .iter()
                .filter(|n| n.node_type == NodeType::LowVolumeNode && n.bin.price_low > price)
                .min_by(|a, b| a.bin.price_mid.partial_cmp(&b.bin.price_mid).unwrap())
                .map(|n| n.bin.price_mid)
        } else {
            self.nodes
                .iter()
                .filter(|n| n.node_type == NodeType::LowVolumeNode && n.bin.price_high < price)
                .max_by(|a, b| a.bin.price_mid.partial_cmp(&b.bin.price_mid).unwrap())
                .map(|n| n.bin.price_mid)
        }
    }
}

// ─── Engine ───────────────────────────────────────────────────────────────────

/// Computes a Visible Range Volume Profile over a slice of [`Bar`]s.
pub struct VrvpEngine {
    /// How many equal-height price buckets to divide the range into.
    pub bin_count: usize,
    /// Fraction of total volume that defines the value area (default 0.70).
    pub value_area_pct: f64,
}

impl VrvpEngine {
    /// Create an engine with sensible defaults.
    pub fn new(bin_count: usize) -> Self {
        Self {
            bin_count,
            value_area_pct: 0.70,
        }
    }

    /// Compute the volume profile from a slice of bars.
    ///
    /// Returns `None` if `bars` is empty or contains no volume data.
    pub fn compute(&self, bars: &[Bar]) -> Option<VrvpProfile> {
        if bars.is_empty() {
            return None;
        }

        // 1. Visible range bounds
        let range_low = bars.iter().map(|b| b.low).fold(f64::MAX, f64::min);
        let range_high = bars.iter().map(|b| b.high).fold(f64::MIN, f64::max);
        if range_high <= range_low {
            return None;
        }

        let bin_size = (range_high - range_low) / self.bin_count as f64;

        // 2. Initialise empty bins
        let mut bins: Vec<VrvpBin> = (0..self.bin_count)
            .map(|i| {
                let low = range_low + i as f64 * bin_size;
                let high = low + bin_size;
                VrvpBin {
                    price_low: low,
                    price_high: high,
                    price_mid: (low + high) / 2.0,
                    volume: 0.0,
                }
            })
            .collect();

        // 3. Distribute each bar's volume across overlapping bins
        let mut total_volume = 0.0_f64;
        for bar in bars {
            let vol = bar.volume.unwrap_or(0.0);
            if vol == 0.0 {
                continue;
            }

            let bar_range = bar.high - bar.low;
            if bar_range == 0.0 {
                // Doji / single-tick bar — put all volume in the close bucket
                let idx = self.price_to_bin_idx(bar.close, range_low, bin_size);
                bins[idx].volume += vol;
            } else {
                for bin in bins.iter_mut() {
                    let overlap_low = bar.low.max(bin.price_low);
                    let overlap_high = bar.high.min(bin.price_high);
                    if overlap_high > overlap_low {
                        let fraction = (overlap_high - overlap_low) / bar_range;
                        bin.volume += vol * fraction;
                    }
                }
            }

            total_volume += vol;
        }

        if total_volume == 0.0 {
            return None;
        }

        // 4. Point of Control
        let poc_idx = bins
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.volume.partial_cmp(&b.volume).unwrap())?
            .0;
        let poc = bins[poc_idx].price_mid;

        // 5. Value area (expand from POC, always adding the higher adjacent bin first)
        let target_volume = total_volume * self.value_area_pct;
        let (val_idx, vah_idx) = self.compute_value_area(&bins, poc_idx, target_volume);

        let vah = bins[vah_idx].price_high;
        let val = bins[val_idx].price_low;
        debug_assert!(val_idx <= vah_idx, "val_idx ({val_idx}) must be <= vah_idx ({vah_idx})");
        let value_area_volume: f64 = bins[val_idx..=vah_idx].iter().map(|b| b.volume).sum();

        // 6. HVN / LVN classification — percentile-based (85th / 15th)
        let mut sorted_volumes: Vec<f64> = bins.iter().map(|b| b.volume).collect();
        sorted_volumes.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let n = sorted_volumes.len();
        let hvn_threshold = sorted_volumes[(n as f64 * 0.85) as usize];
        let lvn_threshold = sorted_volumes[(n as f64 * 0.15) as usize];

        let nodes: Vec<VrvpNode> = bins
            .iter()
            .map(|bin| {
                let node_type = if bin.volume > hvn_threshold {
                    NodeType::HighVolumeNode
                } else if bin.volume <= lvn_threshold {
                    NodeType::LowVolumeNode
                } else {
                    NodeType::Neutral
                };
                VrvpNode {
                    bin: bin.clone(),
                    node_type,
                }
            })
            .collect();

        Some(VrvpProfile {
            bins,
            nodes,
            poc,
            vah,
            val,
            total_volume,
            value_area_volume,
        })
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    fn price_to_bin_idx(&self, price: f64, range_low: f64, bin_size: f64) -> usize {
        let idx = ((price - range_low) / bin_size) as usize;
        idx.min(self.bin_count - 1)
    }

    /// Expand outward from `poc_idx`, always picking the higher-volume neighbour,
    /// until accumulated volume meets `target`.
    fn compute_value_area(
        &self,
        bins: &[VrvpBin],
        poc_idx: usize,
        target: f64,
    ) -> (usize, usize) {
        let mut lower = poc_idx;
        let mut upper = poc_idx;
        let mut accumulated = bins[poc_idx].volume;

        while accumulated < target {
            let next_lower_vol = if lower > 0 { bins[lower - 1].volume } else { 0.0 };
            let next_upper_vol = if upper < bins.len() - 1 {
                bins[upper + 1].volume
            } else {
                0.0
            };

            if next_lower_vol == 0.0 && next_upper_vol == 0.0 {
                break;
            }

            if next_upper_vol > next_lower_vol {
                upper += 1;
                accumulated += bins[upper].volume;
            } else {
                lower -= 1;
                accumulated += bins[lower].volume;
            }
        }

        (lower, upper)
    }
}

// ─── Data fetching ────────────────────────────────────────────────────────────

async fn fetch_bars(
    http: &reqwest::Client,
    symbol: &str,
    timeframe: &str,
    limit: u32,
) -> Result<Vec<Bar>, String> {
    let candles = fetch_bitget_candles(http, symbol, timeframe, &limit.to_string())
        .await
        .map_err(|e| e.to_string())?;
    Ok(candles
        .into_iter()
        .map(|c| Bar {
            time: Utc.timestamp_millis_opt(c.timestamp).single().unwrap_or_else(Utc::now),
            open: c.open,
            high: c.high,
            low: c.low,
            close: c.close,
            volume: Some(c.volume),
            volume_quote: Some(c.quote_volume),
        })
        .collect())
}

// ─── Main computation ─────────────────────────────────────────────────────────

async fn vrvp_main(
    conn: &mut redis::aio::MultiplexedConnection,
    http: &reqwest::Client,
    symbol: &str,
    timeframe: &str,
    candle_count: u32,
    bin_count: usize,
    interval_secs: u64,
) {
    let mut bars = match fetch_bars(http, symbol, timeframe, candle_count).await {
        Ok(b) if b.is_empty() => {
            info!("VRVP[{timeframe}]: no bar data received, skipping");
            return;
        }
        Ok(b) => b,
        Err(e) => {
            log::error!("VRVP[{timeframe}]: fetch_bars error: {e}");
            return;
        }
    };

    bars.sort_by_key(|b| b.time);

    let engine = VrvpEngine::new(bin_count);
    let Some(profile) = engine.compute(&bars) else {
        info!("VRVP[{timeframe}]: could not compute profile (insufficient data)");
        return;
    };

    info!(
        "VRVP[{timeframe}]: POC={:.2}  VAL={:.2}  VAH={:.2}  total_vol={:.2}",
        profile.poc, profile.val, profile.vah, profile.total_volume
    );

    let hvn_count = profile
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::HighVolumeNode)
        .count();
    let lvn_count = profile
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::LowVolumeNode)
        .count();
    info!("VRVP[{timeframe}]: HVN bins={hvn_count}  LVN bins={lvn_count}");

    // Key is timeframe-qualified so multiple instances can coexist in Redis.
    let redis_key = format!("{TRADING_BOT_VRVP}:{timeframe}");
    let serialized = match serde_json::to_string(&profile) {
        Ok(s) => s,
        Err(e) => {
            log::error!("VRVP[{timeframe}]: serialisation error: {e}");
            return;
        }
    };
    let ttl = (interval_secs * 2) as usize;
    if let Err(e) = conn.set_ex::<_, _, ()>(redis_key, serialized, ttl).await {
        log::error!("VRVP[{timeframe}]: Redis write failed: {e}");
    }
}

// ─── Background loop ──────────────────────────────────────────────────────────

/// Runs the VRVP computation on `interval_secs` for the given `timeframe`, `candle_count`,
/// and `bin_count`.
///
/// Spawn one task per timeframe you want to track, e.g.:
/// ```ignore
/// tokio::spawn(vrvp_loop(conn.clone(), "15m", "333", 150, 45));
/// tokio::spawn(vrvp_loop(conn.clone(), "4H",  "500", 100, 1800));
/// tokio::spawn(vrvp_loop(conn.clone(), "1D",  "365",  75, 7200));
/// tokio::spawn(vrvp_loop(conn.clone(), "1W",   "52",  60, 14400));
/// ```
/// Results are stored under `trading_bot:vrvp:<timeframe>`.
pub async fn vrvp_loop(
    mut conn: redis::aio::MultiplexedConnection,
    http: Arc<reqwest::Client>,
    symbol: Arc<str>,
    timeframe: &'static str,
    candle_count: u32,
    bin_count: usize,
    interval_secs: u64,
) {
    let mut interval = time::interval(Duration::from_secs(interval_secs));
    loop {
        interval.tick().await;
        vrvp_main(&mut conn, &http, &symbol, timeframe, candle_count, bin_count, interval_secs).await;
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn bar(low: f64, high: f64, vol: f64) -> Bar {
        Bar {
            time: Utc::now(),
            open: low,
            high,
            low,
            close: (low + high) / 2.0,
            volume: Some(vol),
            volume_quote: None,
        }
    }

    #[test]
    fn test_poc_is_highest_volume_bin() {
        let bars = vec![
            bar(100.0, 110.0, 10.0),
            bar(105.0, 115.0, 50.0), // most volume concentrated 105-115
            bar(108.0, 112.0, 40.0),
            bar(90.0, 100.0, 5.0),
        ];
        let profile = VrvpEngine::new(20).compute(&bars).unwrap();
        // POC must lie within the heavily traded 105-115 region
        assert!(
            profile.poc >= 104.0 && profile.poc <= 116.0,
            "POC {:.2} outside expected range",
            profile.poc
        );
    }

    #[test]
    fn test_value_area_covers_at_least_70_pct() {
        let bars = vec![
            bar(100.0, 200.0, 100.0),
            bar(120.0, 160.0, 200.0),
            bar(130.0, 150.0, 300.0),
        ];
        let profile = VrvpEngine::new(50).compute(&bars).unwrap();
        let va_ratio = profile.value_area_volume / profile.total_volume;
        assert!(
            va_ratio >= 0.70,
            "value area covers only {:.1}%",
            va_ratio * 100.0
        );
    }

    #[test]
    fn test_val_below_vah() {
        let bars = vec![
            bar(50.0, 100.0, 20.0),
            bar(70.0, 90.0, 80.0),
        ];
        let profile = VrvpEngine::new(30).compute(&bars).unwrap();
        assert!(
            profile.val < profile.vah,
            "VAL ({:.2}) should be below VAH ({:.2})",
            profile.val,
            profile.vah
        );
    }

    #[test]
    fn test_empty_bars_returns_none() {
        assert!(VrvpEngine::new(50).compute(&[]).is_none());
    }

    #[test]
    fn test_node_classification_present() {
        let bars: Vec<Bar> = (0..20)
            .map(|i| bar(i as f64 * 10.0, i as f64 * 10.0 + 9.0, if i == 10 { 1000.0 } else { 1.0 }))
            .collect();
        let profile = VrvpEngine::new(40).compute(&bars).unwrap();
        let has_hvn = profile
            .nodes
            .iter()
            .any(|n| n.node_type == NodeType::HighVolumeNode);
        let has_lvn = profile
            .nodes
            .iter()
            .any(|n| n.node_type == NodeType::LowVolumeNode);
        assert!(has_hvn, "expected at least one HVN");
        assert!(has_lvn, "expected at least one LVN");
    }

    // ── Query API tests ───────────────────────────────────────────────────────

    #[test]
    fn test_node_at_returns_hvn_at_dominant_cluster() {
        // Non-overlapping bars: one clear outlier cluster, rest low-volume
        let bars = vec![
            bar(110.0, 112.0, 500.0), // dominant cluster — no overlap with others
            bar(90.0, 100.0, 1.0),
            bar(102.0, 104.0, 1.0),
        ];
        let profile = VrvpEngine::new(50).compute(&bars).unwrap();
        // 111.0 falls squarely inside the dominant cluster
        assert_eq!(profile.node_at(111.0), NodeType::HighVolumeNode);
    }

    #[test]
    fn test_node_at_out_of_range_returns_neutral() {
        let bars = vec![bar(100.0, 200.0, 100.0)];
        let profile = VrvpEngine::new(20).compute(&bars).unwrap();
        assert_eq!(profile.node_at(50.0), NodeType::Neutral);
        assert_eq!(profile.node_at(300.0), NodeType::Neutral);
    }

    #[test]
    fn test_nearest_hvn_above_returns_closest() {
        let bars = vec![
            bar(108.0, 112.0, 400.0), // HVN ~110
            bar(128.0, 132.0, 400.0), // HVN ~130
            bar(100.0, 200.0, 5.0),   // low-volume noise across full range
        ];
        let profile = VrvpEngine::new(100).compute(&bars).unwrap();
        let hvn = profile.nearest_hvn_in_direction(100.0, true);
        assert!(hvn.is_some());
        assert!(hvn.unwrap() < 130.0, "should return closer HVN (~110), not farther one (~130)");
    }

    #[test]
    fn test_nearest_lvn_below_returns_closest() {
        let bars = vec![
            bar(80.0, 82.0, 0.5),    // LVN ~81
            bar(70.0, 72.0, 0.5),    // LVN ~71
            bar(85.0, 100.0, 500.0), // HVN ~92 (dominant)
        ];
        let profile = VrvpEngine::new(100).compute(&bars).unwrap();
        let lvn = profile.nearest_lvn_in_direction(85.0, false);
        assert!(lvn.is_some());
        assert!(lvn.unwrap() > 70.0, "should return closer LVN (~81), not farther one (~71)");
    }

    #[test]
    fn test_nearest_hvn_returns_none_when_no_hvn_in_direction() {
        let bars = vec![
            bar(80.0, 90.0, 500.0),  // HVN here
            bar(100.0, 200.0, 1.0),  // LVN across the rest
        ];
        let profile = VrvpEngine::new(50).compute(&bars).unwrap();
        let hvn = profile.nearest_hvn_in_direction(150.0, true);
        assert!(hvn.is_none());
    }

    #[test]
    fn test_percentile_hvn_count_bounded() {
        let mut bars: Vec<Bar> = (0..40)
            .map(|i| bar(i as f64 * 10.0, i as f64 * 10.0 + 9.0, 10.0))
            .collect();
        bars.push(bar(500.0, 509.0, 10_000.0)); // extreme outlier

        let profile = VrvpEngine::new(200).compute(&bars).unwrap();
        let hvn_count = profile
            .nodes
            .iter()
            .filter(|n| n.node_type == NodeType::HighVolumeNode)
            .count();

        // 15% of 200 bins = max 30 HVN bins
        assert!(hvn_count <= 30, "too many HVN bins: {hvn_count}");
    }
}
