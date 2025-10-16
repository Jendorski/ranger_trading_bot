use log::info;

use crate::{bot::Position, config::Config};
use chrono::{Local, Timelike};

pub struct Helper {
    pub config: Config,
}

impl Helper {
    pub fn from_config() -> Helper {
        let config = Config::from_env().expect("NO CONFIGURATION");
        Self { config }
    }

    pub fn compute_pnl(
        pos: Position,
        entry_price: f64,
        position_size: f64,
        exit_price: f64,
    ) -> f64 {
        let mut pnl = 0.00;

        if !entry_price.is_finite() || !exit_price.is_finite() {
            pnl = 0.00;
        }

        if pos == Position::Long {
            pnl = exit_price - entry_price;
        }

        if pos == Position::Short {
            pnl = entry_price - exit_price;
        }

        if pos == Position::Flat {
            pnl = 0.00;
        }

        let pos_size = position_size;

        if pnl.is_finite() && pos_size.is_finite() {
            return pnl * pos_size;
        }

        return 0.00;
    }

    pub fn position_size(margin: f64, leverage: f64) -> f64 {
        margin * leverage
    }

    pub fn calc_roi(
        &mut self,
        margin: f64,
        entry_price: f64,
        pos: Position,
        position_size: f64,
        exit_price: f64,
    ) -> f64 {
        let pnl = Self::compute_pnl(pos, entry_price, position_size, exit_price);

        let mut roi: f64 = 0.00; // fraction – multiply by 100 for percent

        if pnl.is_finite() && margin.is_finite() {
            roi = (pnl / margin) * 100.0;
        }

        if pnl != 0.00 && margin != 0.00 {
            roi = (pnl / margin) * 100.0;
        }
        roi
    }

    //Function to calculate the amount of the crypto (BTC in this case) bought in the Futures.
    //If your margin, for example is 50 USDT with a leverage of 20, total is 1000 USDT
    //This function then calculates the amount of BTC bought with that 1000 USDT
    pub fn contract_amount(entry_price: f64, margin: f64, leverage: f64) -> f64 {
        let position_size = Self::position_size(margin, leverage);
        let btc_amount = position_size / entry_price;

        btc_amount
    }

    /// Returns **true** iff the supplied `DateTime<Utc>` is exactly midnight (00:00).
    pub fn is_midnight() -> bool {
        let now = Local::now();
        info!("now.minute -> {:2}", now.minute());
        now.hour() == 00 && now.minute() == 0
    }

    /// Percentage PnL of a single trade
    pub fn pnl_percent(entry: f64, exit: f64, leverage: f64) -> f64 {
        if !entry.is_finite() || !exit.is_finite() {
            return 0.00;
        }

        if entry == 0.00 || exit == 0.00 {
            return 0.00;
        }

        let pl = (exit - entry) / entry;

        return pl * leverage * 100.00;
    }
}
