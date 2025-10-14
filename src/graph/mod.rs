use anyhow::Result;
use anyhow::anyhow;
use chrono::{Datelike, Local, Timelike};
use redis::{AsyncCommands, aio::MultiplexedConnection};
use serde_json;
use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::bot::{self};

pub struct Graph {
    //pub btc_traded: f64,
}

impl Graph {
    //mut conn: redis::aio::MultiplexedConnection
    // pub async fn new() -> Result<Self> {
    //     let btc_traded: f64 = 0.029;

    //     // Self::load_position(&mut conn)
    //     //     .await
    //     //     .unwrap_or_else(|_| Position::Flat);

    //     //Ok(Self { btc_traded })
    // }
    /// Percentage PnL of a single trade
    fn pnl_percent(entry: f64, exit: f64) -> f64 {
        (exit - entry) / entry * 100.0
    }

    /// Absolute profit in USD assuming we always invest `notional` dollars at entry.
    fn pnl_absolute(entry: f64, exit: f64, notional: f64) -> f64 {
        let qty = notional / entry; // BTC amount bought/sold
        (exit - entry) * qty // USD profit/loss
    }

    /// Map `(year, week)` → cumulative ROI (as a fraction, e.g., 0.05 = +5 %)
    pub fn cumulative_roi_weekly(positions: &[bot::ClosedPosition]) -> BTreeMap<(i32, u32), f64> {
        let grouped = Self::group_by_week(positions);
        grouped
            .into_iter()
            .map(|(k, pcts)| {
                let mut prod = 1.0;
                for &pct in &pcts {
                    prod *= 1.0 + pct / 100.0;
                }
                (k, prod - 1.0) // subtract the “starting capital”
            })
            .collect()
    }

    /**
     * growth_factor = Π (1 + pnl_percent/100)
    ROI = growth_factor – 1
     */
    /// Cumulative ROI as a fraction (e.g., 0.05 = 5 %) per week
    // fn cumulative_roi_weekly(positions: &[bot::ClosedPosition]) -> HashMap<(i32, u32), f64> {
    //     let mut map: HashMap<(i32, u32), f64> = HashMap::new();

    //     for pos in positions {
    //         let iso = pos.exit_time.iso_week();
    //         let key = (iso.year(), iso.week());
    //         let pct = Self::pnl_percent(pos.entry_price, pos.exit_price);
    //         let factor = 1.0 + pct / 100.0;
    //         *map.entry(key).or_insert(1.0) *= factor; // cumulative product
    //     }

    //     // Convert to ROI fraction (subtract the “1” that represents starting capital)
    //     for v in map.values_mut() {
    //         *v -= 1.0;
    //     }
    //     map
    // }

    /// Same idea, but by calendar month
    // pub fn cumulative_roi_monthly(positions: &[bot::ClosedPosition]) -> HashMap<(i32, u32), f64> {
    //     let mut map: HashMap<(i32, u32), f64> = HashMap::new();

    //     for pos in positions {
    //         let key = (pos.exit_time.year(), pos.exit_time.month());
    //         let pct = Self::pnl_percent(pos.entry_price, pos.exit_price);
    //         let factor = 1.0 + pct / 100.0;
    //         *map.entry(key).or_insert(1.0) *= factor;
    //     }

    //     for v in map.values_mut() {
    //         *v -= 1.0;
    //     }
    //     map
    // }

    /// Same idea, but by calendar month
    pub fn cumulative_roi_monthly(positions: &[bot::ClosedPosition]) -> BTreeMap<(i32, u32), f64> {
        let grouped = Self::group_by_month(positions);
        grouped
            .into_iter()
            .map(|(k, pcts)| {
                let mut prod = 1.0;
                for &pct in &pcts {
                    prod *= 1.0 + pct / 100.0;
                }
                (k, prod - 1.0)
            })
            .collect()
    }

    const NOTIONAL_PER_TRADE: f64 = 50.0; // e.g., $10 k per BTC

    /// ROI per week as a fraction of *total* capital invested that week.
    pub fn roi_weekly_absolute(positions: &[bot::ClosedPosition]) -> HashMap<(i32, u32), f64> {
        let mut profit_map: HashMap<(i32, u32), f64> = HashMap::new();
        let mut cap_map: HashMap<(i32, u32), f64> = HashMap::new();

        for pos in positions {
            let iso = pos.exit_time.iso_week();
            let key = (iso.year(), iso.week());
            let profit =
                Self::pnl_absolute(pos.entry_price, pos.exit_price, Self::NOTIONAL_PER_TRADE);
            *profit_map.entry(key).or_insert(0.0) += profit;
            *cap_map.entry(key).or_insert(0.0) += Self::NOTIONAL_PER_TRADE;
        }

        // ROI = profit / capital invested
        profit_map
            .into_iter()
            .map(|(k, p)| (k, p / cap_map[&k]))
            .collect()
    }

    /// Map `(year, week)` → average % return
    pub fn avg_pnl_weekly(positions: &[bot::ClosedPosition]) -> BTreeMap<(i32, u32), f64> {
        let grouped = Self::group_by_week(positions);
        grouped
            .into_iter()
            .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
            .collect()
    }

    /// Average PnL % for each week (ISO year‑week)
    // pub fn avg_pnl_weekly(positions: &[ClosedPosition]) -> HashMap<(i32, u32), f64> {
    //     Self::group_by_week(positions)
    //         .into_iter()
    //         .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
    //         .collect()
    // }

    /// Map `(year, month)` → average % return
    pub fn avg_pnl_monthly(positions: &[bot::ClosedPosition]) -> BTreeMap<(i32, u32), f64> {
        let grouped = Self::group_by_month(positions);
        grouped
            .into_iter()
            .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
            .collect()
    }

    // /// Average PnL % for each month
    // pub fn avg_pnl_monthly(positions: &[ClosedPosition]) -> HashMap<(i32, u32), f64> {
    //     Self::group_by_month(positions)
    //         .into_iter()
    //         .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
    //         .collect()
    // }

    pub async fn load_all_closed_positions(
        conn: &mut MultiplexedConnection,
    ) -> Result<Vec<bot::ClosedPosition>> {
        let key = "closed_positions";
        // `LRANGE 0 -1` returns the whole list (newest → oldest)
        let raw_jsons: Vec<String> = conn.lrange(key, 0, -1).await?;

        // Deserialize each JSON string into a struct
        raw_jsons
            .into_iter()
            .map(|j| {
                serde_json::from_str::<bot::ClosedPosition>(&j)
                    .map_err(|e| anyhow!("Failed to parse: {}", e))
            })
            .collect()
    }

    /// Returns a map `[(year, week), Vec<pnl_percent>]`
    pub fn group_by_week(positions: &[bot::ClosedPosition]) -> HashMap<(i32, u32), Vec<f64>> {
        let mut map: HashMap<(i32, u32), Vec<f64>> = HashMap::new();
        for pos in positions {
            let iso = pos.exit_time.iso_week(); // ISO‑8601 week (Mon–Sun)
            let key = (iso.year(), iso.week());

            map.entry(key)
                .or_default()
                .push(Self::pnl_percent(pos.entry_price, pos.exit_price));
        }
        map
    }

    /// Returns a map `[(year, month), Vec<pnl_percent>]`
    fn group_by_month(positions: &[bot::ClosedPosition]) -> HashMap<(i32, u32), Vec<f64>> {
        let mut map: HashMap<(i32, u32), Vec<f64>> = HashMap::new();
        for pos in positions {
            let key = (pos.exit_time.year(), pos.exit_time.month());
            map.entry(key)
                .or_default()
                .push(Self::pnl_percent(pos.entry_price, pos.exit_price));
        }
        map
    }

    /// Returns **true** iff the supplied `DateTime<Utc>` is exactly midnight (00:00).
    pub fn is_midnight() -> bool {
        let now = Local::now();
        now.hour() == 0 && now.minute() == 0
    }

    pub async fn prepare_in_logs(
        mut conn: redis::aio::MultiplexedConnection,
    ) -> anyhow::Result<()> {
        let positions = Self::load_all_closed_positions(&mut conn).await?;

        // ------------------------------------------------------------------
        // 1. Average % PnL per week
        // ------------------------------------------------------------------
        println!("--- Avg PnL % per week ---");
        for ((y, w), pct) in Self::avg_pnl_weekly(&positions) {
            println!("{:04}-W{:02}: {:.2} %", y, w, pct);
        }

        // ------------------------------------------------------------------
        // 2. Cumulative ROI per month (as a percent)
        // ------------------------------------------------------------------
        println!("\n--- Cumulative ROI % per month ---");
        for ((y, m), roi) in Self::cumulative_roi_monthly(&positions) {
            println!("{:04}-{:02}: {:.2} %", y, m, roi * 100.0);
        }

        // ------------------------------------------------------------------
        // 3. Absolute ROI (realised capital) per week
        // ------------------------------------------------------------------
        println!("\n--- Absolute‑capital ROI % per week ---");
        for ((y, w), roi) in Self::roi_weekly_absolute(&positions) {
            println!("{:04}-W{:02}: {:.2} %", y, w, roi * 100.0);
        }

        Ok(())
    }
}
