use anyhow::Result;
use anyhow::anyhow;
use chrono::Datelike;
use chrono::Utc;
use redis::{AsyncCommands, aio::MultiplexedConnection};
use serde_json;
use std::collections::BTreeMap;
use std::collections::HashMap;
use uuid::Uuid;

use crate::bot::ClosedPosition;
use crate::bot::Position;
use crate::bot::{self};
use crate::config::Config;
use crate::helper::Helper;
use crate::helper::TRADING_BOT_CLOSE_POSITIONS;
use crate::helper::TRADING_CAPITAL;

pub struct Graph {
    pub config: Config,
}

impl Graph {
    pub fn new() -> Self {
        let config = Config::from_env().expect("NO CONFIGURATION");

        Self { config }
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
                let mut prod = 0.0; //1.0;
                for &pct in &pcts {
                    prod += pct; //1.0 + pct / 100.0;
                }
                (k, prod) //- 1.0 subtract the “starting capital”
            })
            .collect()
    }

    /// Same idea, but by calendar month
    pub fn cumulative_roi_monthly(
        &mut self,
        positions: &[bot::ClosedPosition],
    ) -> BTreeMap<(i32, u32), f64> {
        let grouped = Self::group_by_month(self, positions);
        grouped
            .into_iter()
            .map(|(k, pcts)| {
                let mut prod = 0.0; //1.0;
                for &pct in &pcts {
                    prod += pct; //1.0 + pct / 100.0;
                }
                (k, prod) //- 1.0
            })
            .collect()
    }

    fn load_default_closed_position() -> String {
        let closed = ClosedPosition {
            id: Uuid::nil(),
            pnl: 0.00,
            position: Some(Position::Flat),
            side: Some(Position::Flat),
            entry_price: 0.00,
            entry_time: Utc::now(),
            exit_price: 0.00,
            exit_time: Utc::now(),
            quantity: Some(0.00),
            sl: Some(0.00),
            roi: Some(0.00),
            leverage: Some(0.00),
            margin: Some(0.00),
            order_id: None,
        };

        closed.as_str()
    }

    pub async fn load_all_closed_positions(
        conn: &mut MultiplexedConnection,
    ) -> Result<Vec<bot::ClosedPosition>> {
        let key = TRADING_BOT_CLOSE_POSITIONS; //SCALPER_CLOSED_POSITIONS TRADING_BOT_CLOSE_POSITIONS

        let mut raw_jsons = [Self::load_default_closed_position()].to_vec();

        let exists: usize = conn.exists(key).await?;

        // `LRANGE 0 -1` returns the whole list (newest → oldest)
        if exists != 0 {
            raw_jsons = conn.lrange(key, 0, -1).await?;
        }

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
                    //pos.leverage.unwrap_or(self.config.leverage),
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
                    //pos.leverage.unwrap_or(self.config.leverage),
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

        let margin = pos.margin.unwrap_or(self.config.margin);

        let mut roi: f64 = 0.00; // fraction – multiply by 100 for percent

        //if pnl != 0.00 && margin != 0.00 {
        if pnl.is_finite() && margin.is_finite() && pnl != 0.00 && margin != 0.00 {
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

    pub async fn prepare_cumulative_weekly_monthly(
        &mut self,
        mut conn: redis::aio::MultiplexedConnection,
    ) -> anyhow::Result<()> {
        let mut positions = Self::load_all_closed_positions(&mut conn).await?;

        let margin_config = self.config.margin;

        // 2️⃣ Sort chronologically – we use exit_time as the definitive moment of closure
        positions.sort_by_key(|p| p.exit_time);

        // ------------------------------------------------------------------
        // 1. Average % PnL per week
        // ------------------------------------------------------------------
        // println!("--- Avg PnL % per week ---");

        println!(
            "{:<36} {:<36} {:<6} {:>10} {:>10} {:>4.3} {:>4.3}",
            "Date", "ID", "Side", "Entry", "Exit", "PnL ($)", "ROI (%)"
        );
        let mut total_pnl: f64 = 0.0;

        let exists: usize = conn.exists(TRADING_CAPITAL).await?;

        let mut raw_margin = String::from("0.00");

        if exists != 0 {
            raw_margin = conn.get(TRADING_CAPITAL).await?;
        }

        let mut total_margin: f64 =
            serde_json::from_str::<Option<f64>>(&raw_margin)?.unwrap_or_else(|| self.config.margin);

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
        println!("\nCumulative realised PnL: ${:.2}", total_pnl);
        println!(
            "Cumulative margin used (across all trades): ${:.2},",
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
            println!("{:04}-{:02}: {:.2} %", y, m, roi); //* 100.0
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
