use crate::exchange::bitget::Candle;
use crate::{bot::Position, config::Config};
use anyhow::{anyhow, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local, TimeZone, Timelike, Utc};
use rust_decimal::prelude::{FromPrimitive as _, ToPrimitive};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct InputCandle {
    #[serde(rename = "Timestamp")]
    timestamp: f64,
    #[serde(rename = "Open")]
    open: f64,
    #[serde(rename = "High")]
    high: f64,
    #[serde(rename = "Low")]
    low: f64,
    #[serde(rename = "Close")]
    close: f64,
    #[serde(rename = "Volume")]
    volume: f64,
}

// pub const TRADING_SCALPER_BOT_ACTIVE: &str = "trading_scalper_bot::active";
// pub const TRADIN_SCALPER_BOT_POSITION: &str = "trading_scalper_bot::position";
// pub const SCALPER_CLOSED_POSITIONS: &str = "scalper_closed_positions";
pub const TRADING_BOT_ZONES: &str = "trading_bot:zones";
pub const TRADING_BOT_POSITION: &str = "trading_bot:position";
pub const TRADING_BOT_ACTIVE: &str = "trading::active";
pub const TRADING_BOT_CLOSE_POSITIONS: &str = "closed_positions";
pub const TRADING_CAPITAL: &str = "trading_capital";
pub const TRADING_PARTIAL_PROFIT_TARGET: &str = "trading_partial_profit_target";
pub const TRADING_BOT_LOSS_COUNT: &str = "trading_bot:loss_count";
pub const TRADING_BOT_SMART_MONEY_CONCEPTS_NEXT_CALL: &str =
    "trading_bot:smart_money_concepts_next_call";
pub const TRADING_BOT_RECOMMENDED_CALL: &str = "trading_bot:recommended_call";
pub const WEEKLY_CANDLES: &'static str = "weekly_candles";
pub const WEEKLY_ICHIMOKU: &'static str = "weekly_ichimoku";
pub const LAST_25_WEEKLY_ICHIMOKU_SPANS: &'static str = "last_25_weekly_ichimoku_spans";

pub struct Helper {
    pub config: Config,
}

/// A target that says “close X % of my remaining qty when the market reaches Y”.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct PartialProfitTarget {
    /// The price at which we want to take profit.
    pub target_price: Decimal,

    /// Fraction of *remaining* quantity to close (0.0–1.0).
    pub fraction: Decimal,

    //The SL the Position MUST be in when the target price is hit
    pub sl: Option<Decimal>,

    pub size_btc: Decimal,
}

impl fmt::Display for PartialProfitTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TP @ {:.2}  →  SL @ {:.2}  (close {:.0}% of remaining)",
            self.target_price,
            self.sl.unwrap_or(dec!(0.00)),
            self.fraction.to_f64().unwrap() * 100.0
        )
    }
}

impl Helper {
    pub fn from_config() -> Helper {
        let config = Config::from_env().expect("NO CONFIGURATION");
        Self { config }
    }

    pub fn compute_pnl(
        pos: Position,
        entry_price: Decimal,
        position_size: Decimal,
        exit_price: Decimal,
    ) -> Decimal {
        let mut pnl_diff = dec!(0.00);

        if !entry_price.is_sign_positive() || !exit_price.is_sign_positive() {
            return dec!(0.00);
        }

        if pos == Position::Long && exit_price != dec!(0.00) && entry_price != dec!(0.00) {
            pnl_diff = exit_price - entry_price;
        }

        if pos == Position::Short && exit_price != dec!(0.00) && entry_price != dec!(0.00) {
            pnl_diff = entry_price - exit_price;
        }

        if pos == Position::Flat {
            pnl_diff = dec!(0.00);
        }

        let pos_size = position_size;

        if pnl_diff.is_sign_positive() && pos_size.is_sign_positive() {
            return pnl_diff * pos_size;
        }

        return dec!(0.00);
    }

    pub fn position_size(margin: Decimal, leverage: Decimal) -> Decimal {
        margin * leverage
    }

    pub fn calc_roi(
        &mut self,
        margin: Decimal,
        entry_price: Decimal,
        pos: Position,
        position_size: Decimal,
        exit_price: Decimal,
    ) -> Decimal {
        let pnl = Self::compute_pnl(pos, entry_price, position_size, exit_price);

        let mut roi: Decimal = dec!(0.00); // fraction – multiply by 100 for percent

        if pnl.is_sign_positive() && margin.is_sign_positive() {
            roi = (pnl / margin) * dec!(100.0);
        }

        if pnl != dec!(0.00) && margin != dec!(0.00) {
            roi = (pnl / margin) * dec!(100.0);
        }
        roi
    }

    //Function to calculate the amount of the crypto (BTC in this case) bought in the Futures.
    //If your margin, for example is 50 USDT with a leverage of 20, total is 1000 USDT
    //This function then calculates the amount of BTC bought with that 1000 USDT
    pub fn contract_amount(entry_price: Decimal, margin: Decimal, leverage: Decimal) -> Decimal {
        let position_size = Self::position_size(margin, leverage);
        let btc_amount = position_size / entry_price;

        btc_amount
    }

    /// Returns **true** iff the supplied `DateTime<Utc>` is exactly midnight (00:00).
    pub fn is_midnight() -> bool {
        let now = Local::now();
        now.hour() == 00 && now.minute() == 0
    }

    /// Percentage PnL of a single trade
    pub fn pnl_percent(entry: f64, exit: f64, pos: Position) -> f64 {
        if !entry.is_finite() || !exit.is_finite() {
            return 0.00;
        }

        if entry == 0.00 || exit == 0.00 {
            return 0.00;
        }

        let mut pl_diff = 0.00;

        if pos == Position::Long {
            pl_diff = exit - entry;
        }

        if pos == Position::Short {
            pl_diff = entry - exit;
        }

        if pos == Position::Flat {
            //pl = 0.00;
            pl_diff = 0.00
        }

        let pl = pl_diff / entry;

        //Wondering why I added leverage here in the first place
        return pl * 100.00; //leverage *
    }

    pub fn truncate_to_1_dp(val: f64) -> f64 {
        (val * 10.0).trunc() / 10.0
    }

    pub fn stop_loss_price(
        entry_price: Decimal,
        margin: Decimal,
        leverage: Decimal,
        risk_pct: Decimal,
        pos: Position,
    ) -> Decimal {
        let desired_loss = margin * risk_pct; // $4.65
        let position_size = Helper::position_size(margin, leverage);
        let delta_price = (desired_loss / position_size) * entry_price; //desired_loss / quantity; // how many dollars of price change

        if pos == Position::Long {
            return entry_price - delta_price;
        }

        if pos == Position::Short {
            return entry_price + delta_price;
        }

        dec!(0.00)
    }

    //Function to trigger Stop Loss
    pub fn ssl_hit(current_price: Decimal, side: Position, sl: Decimal) -> bool {
        if side == Position::Long {
            return current_price <= sl;
        }

        if side == Position::Short {
            return current_price >= sl;
        }

        false
    }

    fn tp_prices(
        ranger_price_difference: Decimal,
        entry_price: Decimal,
        tp_counts: usize,
        pos: Position,
    ) -> Vec<Decimal> {
        let ranger_price_difference = ranger_price_difference;

        let mut count = 0;
        let mut tp = entry_price;

        let mut tp_pr: Vec<Decimal> = Vec::with_capacity(tp_counts);

        while count < tp_counts {
            if pos == Position::Long {
                tp += ranger_price_difference;
                tp_pr.push(tp);
            }
            if pos == Position::Short {
                tp -= ranger_price_difference;
                tp_pr.push(tp);
            }

            count += 1;
        }

        tp_pr
    }

    pub fn f64_to_decimal(val: f64) -> Decimal {
        Decimal::from_f64(val).unwrap()
    }

    pub fn decimal_to_f64(val: Decimal) -> f64 {
        val.to_f64().unwrap()
    }

    pub fn build_profit_targets(
        entry_price: Decimal,
        margin: Decimal,
        leverage: Decimal,
        ranger_price_difference: Decimal,
        pos: Position,
    ) -> Vec<PartialProfitTarget> {
        // assert_eq!(tp_prices.len(), fractions.len());
        // assert_eq!(fractions.iter().copied().sum::<Decimal>(), dec!(1));

        // BTC precision (e.g. 5 or 6)
        let size_precision: u32 = 5;

        let tp_counts: usize = 4;
        let tp_prices: Vec<Decimal> =
            Helper::tp_prices(ranger_price_difference, entry_price, tp_counts, pos);

        let fractions: &[Decimal] = &[dec!(0.20), dec!(0.30), dec!(0.30), dec!(0.20)];

        // Total notional
        let notional = margin * leverage;

        // Total position size in BTC
        let total_size = (notional / entry_price).round_dp(size_precision);

        let mut remaining = total_size;
        let mut ladder = Vec::with_capacity(tp_prices.len());

        for i in 0..tp_prices.len() {
            let is_last = i == tp_prices.len() - 1;

            let size = if is_last {
                // absorb rounding remainder
                remaining
            } else {
                let raw = (total_size * fractions[i])
                    .round_dp_with_strategy(size_precision, rust_decimal::RoundingStrategy::ToZero);

                remaining -= raw;
                raw
            };

            // ---- SL LOGIC ----
            let next_sl = if is_last {
                None
            } else if i == 0 {
                // After TP1 → SL moves to entry
                Some(entry_price)
            } else {
                // After TPn → SL moves to previous TP price
                Some(tp_prices[i - 1])
            };

            ladder.push(PartialProfitTarget {
                target_price: tp_prices[i],
                fraction: fractions[i],
                size_btc: size,
                sl: next_sl,
            });
        }

        ladder
    }

    // pub fn build_profit_targets(
    //     entry_price: f64,
    //     ranger_price_difference: f64,
    //     pos: Position,
    // ) -> Vec<PartialProfitTarget> {
    //     let tp_counts: usize = 4;

    //     let fracs: [f64; 4] = [0.20, 0.30, 0.30, 0.20];

    //     let tp_prices = Helper::tp_prices(ranger_price_difference, entry_price, tp_counts, pos);

    //     // Default fraction = 25 % if not supplied.
    //     let default_frac = 0.25;

    //     let mut targets = Vec::with_capacity(tp_prices.len());

    //     for (i, &tp) in tp_prices.iter().enumerate() {
    //         // Determine the new stop‑loss after this TP.
    //         let new_sl = if i == 0 {
    //             entry_price // move SL to entry price
    //                         //(entry_price + tp) / 2.0 //the new Stop Loss is now calculated as the midpoint (equidistance) between the entry_price and the target price
    //         } else if i == 1 {
    //             tp_prices[0] //entry_price // TP2 → SL is moved to TP1
    //         } else {
    //             tp_prices[i - 2] // TP3+ → the target two steps before
    //         };

    //         let fraction = fracs.get(i).copied().unwrap_or(default_frac);

    //         targets.push(PartialProfitTarget {
    //             target_price: tp,
    //             sl: new_sl,
    //             fraction,
    //         });
    //     }

    //     targets
    // }

    pub fn funding_multiplier(funding_rate: f64, pos: Position) -> Decimal {
        let scale = 800.0; // Adjust sensitivity
        let mut multiplier = 1.0;

        if pos == Position::Long {
            multiplier = 1.0 - (funding_rate * scale);
        } else if pos == Position::Short {
            multiplier = 1.0 + (funding_rate * scale);
        }

        // Clamp between 0.5 and 1.5 to avoid extreme position sizing
        Helper::f64_to_decimal(multiplier.clamp(0.5, 1.5))
    }

    pub fn extract_into_weekly_candle(path: &str, output_path: &str) -> Result<()> {
        println!("Reading {}...", path);
        if !Path::new(path).exists() {
            return Err(anyhow!("File {} not found", path));
        }

        let file = File::open(path)?;
        let mut rdr = csv::Reader::from_reader(file);

        println!("Processing candles...");

        // Map of Week Ending Timestamp -> List of candles in that week
        // We accumulate aggregate stats directly to avoid storing all candles in memory if possible?
        // But to get 'first' and 'last', we need to know order.
        // The input is presumed sorted by timestamp?
        // If not, we should store them.
        // Given the Python script loads all into DF, memory isn't a huge constraint (385MB file).
        // Storing structs might take ~4-500MB.
        // Let's store intermediate aggregates per week.

        struct WeeklyAgg {
            min_ts: i64,
            max_ts: i64,
            open: f64,  // Open of candle with min_ts
            close: f64, // Close of candle with max_ts
            high: f64,
            low: f64,
            volume: f64,
            quote_volume: f64,
        }

        let mut weekly_data: BTreeMap<i64, WeeklyAgg> = BTreeMap::new();

        for result in rdr.deserialize() {
            let record: InputCandle = result?;

            // Convert timestamp (f64) to i64
            let ts_i64 = record.timestamp as i64;

            // Convert timestamp to UTC DateTime
            // Handle potential errors if timestamp is invalid? assuming valid.
            let dt = Utc.timestamp_opt(ts_i64, 0).unwrap();

            // Find the "Weekly" bin (Sunday)
            // Pandas 'W' (Weekly, Sunday). Alignment is typically End of Week (Sunday).
            // Timestamps on Sunday belong to that Sunday.
            // Timestamps on Mon-Sat belong to next Sunday.

            // number_from_monday: Mon=1, Sun=7.
            // If Sun(7): 7-7=0. Add 0 days. Target = Today.
            // If Mon(1): 7-1=6. Add 6 days. Target = Next Sunday.
            let days_until_sunday = (7 - dt.weekday().number_from_monday()) % 7;
            let target_date = dt.date_naive() + ChronoDuration::days(days_until_sunday as i64);

            let bin_dt = Utc.from_utc_datetime(&target_date.and_hms_opt(0, 0, 0).unwrap());
            let bin_ts = bin_dt.timestamp();

            let quote_vol = record.volume * record.close;

            weekly_data
                .entry(bin_ts)
                .and_modify(|agg| {
                    // Update High/Low/Vol
                    if record.high > agg.high {
                        agg.high = record.high;
                    }
                    if record.low < agg.low {
                        agg.low = record.low;
                    }
                    agg.volume += record.volume;
                    agg.quote_volume += quote_vol;

                    // Update Open if this record is earlier
                    if ts_i64 < agg.min_ts {
                        agg.min_ts = ts_i64;
                        agg.open = record.open;
                    }

                    // Update Close if this record is later
                    if ts_i64 > agg.max_ts {
                        agg.max_ts = ts_i64;
                        agg.close = record.close;
                    }
                })
                .or_insert(WeeklyAgg {
                    min_ts: ts_i64,
                    max_ts: ts_i64,
                    open: record.open,
                    close: record.close,
                    high: record.high,
                    low: record.low,
                    volume: record.volume,
                    quote_volume: quote_vol,
                });
        }

        println!(
            "Resampling to weekly... ({} weeks found)",
            weekly_data.len()
        );
        println!("Saving to {}...", output_path);

        let output_file = File::create(output_path)?;
        let mut wtr = csv::Writer::from_writer(output_file);

        for (ts, agg) in weekly_data {
            wtr.serialize(Candle {
                timestamp: ts,
                open: agg.open,
                high: agg.high,
                low: agg.low,
                close: agg.close,
                volume: agg.volume,
                quote_volume: agg.quote_volume,
            })?;
        }

        wtr.flush()?;
        println!("Transformation complete!");

        Ok(())
    }

    pub fn read_candles_from_csv(file_path: &str) -> Result<Vec<Candle>, Box<dyn Error>> {
        let file = File::open(file_path)?;
        let mut rdr = csv::Reader::from_reader(file);
        let mut candles = Vec::new();
        for result in rdr.deserialize() {
            let candle: Candle = result?;
            candles.push(candle);
        }
        Ok(candles)
    }
}
