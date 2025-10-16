use anyhow::Result;
use anyhow::anyhow;
use chrono::NaiveDate;
use chrono::{Datelike, Local, Timelike};
use redis::{AsyncCommands, aio::MultiplexedConnection};
use serde_json;
use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::bot::ClosedPosition;
use crate::bot::{self};
use crate::config::Config;
use crate::helper::Helper;

pub struct Graph {
    pub config: Config,
}

#[derive(Debug, Clone)]
pub struct WeeklyRoi {
    pub week_start: NaiveDate,
    pub cumulative_roi: f64,
    pub position_count: usize,
}

#[derive(Debug, Clone)]
pub struct MonthlyRoi {
    pub month: (i32, u32), // (year, month)
    pub cumulative_roi: f64,
    pub position_count: usize,
}

impl Graph {
    pub fn new() -> Self {
        let config = Config::from_env().expect("NO CONFIGURATION");

        Self { config }
    }

    pub fn calculate_weekly_roi(positions: &[ClosedPosition]) -> Vec<WeeklyRoi> {
        let mut weekly_map: BTreeMap<(i32, u32, u32), (f64, usize)> = BTreeMap::new();

        for position in positions {
            if let Some(roi) = position.roi {
                let date = position.exit_time.date_naive();
                let iso_week = date.iso_week();
                let key = (date.year(), iso_week.week(), 0); // 0 for consistency

                let entry = weekly_map.entry(key).or_insert((0.0, 0));
                entry.0 += roi;
                entry.1 += 1;
            }
        }

        weekly_map
            .into_iter()
            .map(|((year, week, _), (total_roi, count))| {
                let week_start = NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon)
                    .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, 1, 1).unwrap());

                WeeklyRoi {
                    week_start,
                    cumulative_roi: total_roi,
                    position_count: count,
                }
            })
            .collect()
    }

    pub fn calculate_monthly_roi(positions: &[ClosedPosition]) -> Vec<MonthlyRoi> {
        let mut monthly_map: BTreeMap<(i32, u32), (f64, usize)> = BTreeMap::new();

        for position in positions {
            if let Some(roi) = position.roi {
                let date = position.exit_time.date_naive();
                let key = (date.year(), date.month());

                let entry = monthly_map.entry(key).or_insert((0.0, 0));
                entry.0 += roi;
                entry.1 += 1;
            }
        }

        monthly_map
            .into_iter()
            .map(|((year, month), (total_roi, count))| MonthlyRoi {
                month: (year, month),
                cumulative_roi: total_roi,
                position_count: count,
            })
            .collect()
    }
    /// Map `(year, week)` → cumulative ROI (as a fraction, e.g., 0.05 = +5 %)
    pub fn cumulative_roi_weekly(
        &mut self,
        positions: &[bot::ClosedPosition],
    ) -> BTreeMap<(i32, u32), f64> {
        let grouped = Self::group_by_week(self, positions);
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
    pub fn cumulative_roi_monthly(
        &mut self,
        positions: &[bot::ClosedPosition],
    ) -> BTreeMap<(i32, u32), f64> {
        let grouped = Self::group_by_month(self, positions);
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

    // const NOTIONAL_PER_TRADE: f64 = 50.0; // e.g., $10 k per BTC

    /// ROI per week as a fraction of *total* capital invested that week.
    // pub fn roi_weekly_absolute(positions: &[bot::ClosedPosition]) -> HashMap<(i32, u32), f64> {
    //     let mut profit_map: HashMap<(i32, u32), f64> = HashMap::new();
    //     let mut cap_map: HashMap<(i32, u32), f64> = HashMap::new();

    //     for pos in positions {
    //         let iso = pos.exit_time.iso_week();
    //         let key = (iso.year(), iso.week());
    //         let profit =
    //             Self::pnl_absolute(pos.entry_price, pos.exit_price, Self::NOTIONAL_PER_TRADE);
    //         *profit_map.entry(key).or_insert(0.0) += profit;
    //         *cap_map.entry(key).or_insert(0.0) += Self::NOTIONAL_PER_TRADE;
    //     }

    //     // ROI = profit / capital invested
    //     profit_map
    //         .into_iter()
    //         .map(|(k, p)| (k, p / cap_map[&k]))
    //         .collect()
    // }

    /// Map `(year, week)` → average % return
    // pub fn avg_pnl_weekly(positions: &[bot::ClosedPosition]) -> BTreeMap<(i32, u32), f64> {
    //     let grouped = Self::group_by_week(positions);
    //     grouped
    //         .into_iter()
    //         .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
    //         .collect()
    // }

    /// Average PnL % for each week (ISO year‑week)
    // pub fn avg_pnl_weekly(positions: &[ClosedPosition]) -> HashMap<(i32, u32), f64> {
    //     Self::group_by_week(positions)
    //         .into_iter()
    //         .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
    //         .collect()
    // }

    /// Map `(year, month)` → average % return
    // pub fn avg_pnl_monthly(positions: &[bot::ClosedPosition]) -> BTreeMap<(i32, u32), f64> {
    //     let grouped = Self::group_by_month(positions);
    //     grouped
    //         .into_iter()
    //         .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
    //         .collect()
    // }

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
        // info!("raws: {:.?}", raw_jsons);

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
    pub fn group_by_week(
        &mut self,
        positions: &[bot::ClosedPosition],
    ) -> HashMap<(i32, u32), Vec<f64>> {
        let mut map: HashMap<(i32, u32), Vec<f64>> = HashMap::new();
        for pos in positions {
            let iso = pos.exit_time.iso_week(); // ISO‑8601 week (Mon–Sun)
            let key = (iso.year(), iso.week());

            if pos.entry_price != 0.0 && pos.exit_price != 0.0 {
                let pnl_percent = Helper::pnl_percent(
                    pos.entry_price,
                    pos.exit_price,
                    pos.leverage.unwrap_or(self.config.leverage),
                    pos.position.unwrap_or(bot::Position::Flat),
                );
                map.entry(key).or_default().push(pnl_percent);
            }
        }
        map
    }

    /// Returns a map `[(year, month), Vec<pnl_percent>]`
    fn group_by_month(
        &mut self,
        positions: &[bot::ClosedPosition],
    ) -> HashMap<(i32, u32), Vec<f64>> {
        let mut map: HashMap<(i32, u32), Vec<f64>> = HashMap::new();
        for pos in positions {
            let key = (pos.exit_time.year(), pos.exit_time.month());

            if pos.entry_price != 0.00 && pos.exit_price != 0.00 {
                let pnl_percent = Helper::pnl_percent(
                    pos.entry_price,
                    pos.exit_price,
                    pos.leverage.unwrap_or(self.config.leverage),
                    pos.position.unwrap_or(bot::Position::Flat),
                );
                map.entry(key).or_default().push(pnl_percent);
            }
        }
        map
    }

    /// PnL and ROI relative to the margin you actually put up.
    fn pnl_and_roi(&mut self, pos: &bot::ClosedPosition) -> (f64, f64) {
        let qty = Helper::contract_amount(
            pos.entry_price,
            pos.margin.unwrap_or(self.config.margin),
            pos.leverage.unwrap_or(self.config.leverage),
        );

        let pnl = Helper::compute_pnl(
            pos.position.unwrap_or(bot::Position::Flat),
            pos.entry_price,
            pos.quantity.unwrap_or(qty),
            pos.exit_price,
        );

        let margin = pos.margin.unwrap_or(self.config.margin); //Self::margin_used(pos, leverage);

        let mut roi: f64 = 0.00; // fraction – multiply by 100 for percent

        //if pnl != 0.00 && margin != 0.00 {
        if pnl.is_finite() && margin.is_finite() {
            roi = Helper::calc_roi(
                &mut Helper::from_config(),
                margin,
                pos.entry_price,
                pos.position.unwrap_or(bot::Position::Flat),
                pos.quantity.unwrap_or(qty),
                pos.exit_price,
            )
        }
        (pnl, roi)
    }

    // pub async fn all_trade_compute(
    //     &mut self,
    //     mut conn: redis::aio::MultiplexedConnection,
    // ) -> anyhow::Result<()> {
    //     let positions = Self::load_all_closed_positions(&mut conn).await?;
    //     let margin_config = self.config.margin;

    //     println!(
    //         "{:<36} {:<36} {:<6} {:>10} {:>10} {:>12} {:>12}",
    //         "Date", "ID", "Side", "Entry", "Exit", "PnL ($)", "ROI (%)"
    //     );
    //     let mut total_pnl: f64 = 0.0;
    //     let mut total_margin: f64 = 0.0;

    //     for pos in &positions {
    //         let (pnl, roi) = Self::pnl_and_roi(self, pos);
    //         println!(
    //             "{:36} {:<36} {:<6} {:>10.2} {:>10.2} {:>12.2} {:>12.5} %",
    //             pos.exit_time.format("[%Y-%m-%d][%H:%M:%S]"),
    //             pos.id,
    //             format!("{:?}", pos.position),
    //             pos.entry_price,
    //             pos.exit_price,
    //             pnl,
    //             roi
    //         );

    //         total_pnl += pnl;
    //         total_margin += pos.margin.unwrap_or(margin_config);
    //     }

    //     // ----- Aggregated results --------------------------------------------
    //     println!("\nTotal realised PnL: ${:.2}", total_pnl);
    //     println!(
    //         "Total margin used (across all trades): ${:.2}",
    //         total_margin
    //     );

    //     let overall_roi = if total_margin != 0.0 {
    //         total_pnl / total_margin
    //     } else {
    //         0.0
    //     };
    //     println!(
    //         "Overall ROI on the capital you actually put in: {:.2}%",
    //         overall_roi * 100.0
    //     );

    //     Ok(())
    // }

    pub async fn prepare_cumulative_weekly_monthly(
        &mut self,
        mut conn: redis::aio::MultiplexedConnection,
    ) -> anyhow::Result<()> {
        let positions = Self::load_all_closed_positions(&mut conn).await?;

        let margin_config = self.config.margin;

        // ------------------------------------------------------------------
        // 1. Average % PnL per week
        // ------------------------------------------------------------------
        // println!("--- Avg PnL % per week ---");

        println!(
            "{:<36} {:<36} {:<6} {:>10} {:>10} {:>4.3} {:>4.3}",
            "Date", "ID", "Side", "Entry", "Exit", "PnL ($)", "ROI (%)"
        );
        let mut total_pnl: f64 = 0.0;
        let mut total_margin: f64 = 0.0;

        for pos in &positions {
            let (pnl, roi) = Self::pnl_and_roi(self, pos);
            println!(
                "{:36} {:<36} {:<6} {:>10.2} {:>10.2} {:>4.3} {:>4.3} %",
                pos.exit_time.format("[%Y-%m-%d][%H:%M:%S]"),
                pos.id,
                format!("{:?}", pos.position),
                pos.entry_price,
                pos.exit_price,
                pnl,
                roi
            );

            total_pnl += pnl;
            total_margin += pos.margin.unwrap_or(margin_config);
        }

        // ----- Aggregated results --------------------------------------------
        println!("\n------------------------------------------------------------------------");
        println!("\nTotal realised PnL: ${:.2}", total_pnl);
        println!(
            "Total margin used (across all trades): ${:.2}",
            total_margin
        );

        let overall_roi = if total_margin.is_finite() && total_pnl.is_finite() {
            //total_margin != 0.0
            total_pnl / total_margin
        } else {
            0.0
        };
        println!(
            "Overall ROI on the capital you actually put in: {:.2}%",
            overall_roi * 100.0
        );

        println!("--- Cumulative ROI % per week ---");
        //((y, w), pct)
        for ((y, w), pct) in Self::cumulative_roi_weekly(self, &positions) {
            println!("{:04}-W{:02}: {:.2} %", y, w, pct);
        }

        // ------------------------------------------------------------------
        // 2. Cumulative ROI per month (as a percent)
        // ------------------------------------------------------------------
        println!("\n--- Cumulative ROI % per month ---"); //((y, m), roi)
        for ((y, m), roi) in Self::cumulative_roi_monthly(self, &positions) {
            println!("{:04}-{:02}: {:.2} %", y, m, roi * 100.0);
        }
        println!("\n------------------------------------------------------------------------");

        // ------------------------------------------------------------------
        // 3. Absolute ROI (realised capital) per week
        // ------------------------------------------------------------------
        // println!("\n--- Absolute‑capital ROI % per week ---");
        // for ((y, w), roi) in Self::roi_weekly_absolute(&positions) {
        //     println!("{:04}-W{:02}: {:.2} %", y, w, roi * 100.0);
        // }

        Ok(())
    }
}
