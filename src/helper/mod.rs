use crate::{bot::Position, config::Config};
use chrono::{Local, Timelike};
use std::fmt;

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

pub struct Helper {
    pub config: Config,
}

/// A target that says “close X % of my remaining qty when the market reaches Y”.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct PartialProfitTarget {
    /// The price at which we want to take profit.
    pub target_price: f64,

    /// Fraction of *remaining* quantity to close (0.0–1.0).
    pub fraction: f64,

    //The SL the Position MUST be in when the target price is hit
    pub sl: f64,
}

impl fmt::Display for PartialProfitTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TP @ {:.2}  →  SL @ {:.2}  (close {:.0}% of remaining)",
            self.target_price,
            self.sl,
            self.fraction * 100.0
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
        entry_price: f64,
        position_size: f64,
        exit_price: f64,
    ) -> f64 {
        let mut pnl_diff = 0.00;

        if !entry_price.is_finite() || !exit_price.is_finite() {
            return 0.00;
        }

        if pos == Position::Long && exit_price != 0.00 && entry_price != 0.00 {
            pnl_diff = exit_price - entry_price;
        }

        if pos == Position::Short && exit_price != 0.00 && entry_price != 0.00 {
            pnl_diff = entry_price - exit_price;
        }

        if pos == Position::Flat {
            pnl_diff = 0.00;
        }

        let pos_size = position_size;

        if pnl_diff.is_finite() && pos_size.is_finite() {
            return pnl_diff * pos_size;
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

        let mut pl = 0.00;
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

        pl = pl_diff / entry;

        //Wondering why I added leverage here in the first place
        return pl * 100.00; //leverage *
    }

    pub fn truncate_to_1_dp(val: f64) -> f64 {
        (val * 10.0).trunc() / 10.0
    }

    pub fn stop_loss_price(
        entry_price: f64,
        margin: f64,
        leverage: f64,
        risk_pct: f64,
        pos: Position,
    ) -> f64 {
        let desired_loss = margin * risk_pct; // $4.65
        let position_size = Helper::position_size(margin, leverage);
        let delta_price = (desired_loss / position_size) * entry_price; //desired_loss / quantity; // how many dollars of price change

        if pos == Position::Long {
            return entry_price - delta_price;
        }

        if pos == Position::Short {
            return entry_price + delta_price;
        }

        0.00
    }

    //Function to trigger Stop Loss
    pub fn ssl_hit(current_price: f64, side: Position, sl: f64) -> bool {
        if side == Position::Long {
            return current_price <= sl;
        }

        if side == Position::Short {
            return current_price >= sl;
        }

        false
    }

    fn tp_prices(
        ranger_price_difference: f64,
        entry_price: f64,
        tp_counts: usize,
        pos: Position,
    ) -> Vec<f64> {
        let ranger_price_difference = ranger_price_difference;

        let mut count = 0;
        let mut tp = entry_price;

        let mut tp_pr: Vec<f64> = Vec::with_capacity(tp_counts);

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

    pub fn build_profit_targets(
        entry_price: f64,
        ranger_price_difference: f64,
        pos: Position,
    ) -> Vec<PartialProfitTarget> {
        let tp_counts: usize = 4;

        let fractions: Option<&[f64]> = Some(&[0.50, 0.25, 0.15, 0.10]);

        let tp_prices = Helper::tp_prices(ranger_price_difference, entry_price, tp_counts, pos);

        // Default fraction = 25 % if not supplied.
        let default_frac = 0.25;
        let fracs: Vec<f64> = match fractions {
            Some(f) => f.iter().copied().collect(),
            None => vec![default_frac; tp_prices.len()],
        };

        let mut targets = Vec::with_capacity(tp_prices.len());

        for (i, &tp) in tp_prices.iter().enumerate() {
            // Determine the new stop‑loss after this TP.
            let new_sl = if i == 0 {
                entry_price // move SL to entry price
            } else if i == 1 {
                tp_prices[0] //entry_price // TP2 → SL is moved to TP1
            } else {
                tp_prices[i - 2] // TP3+ → the target two steps before
            };

            let fraction = fracs.get(i).copied().unwrap_or(default_frac);

            targets.push(PartialProfitTarget {
                target_price: tp,
                sl: new_sl,
                fraction,
            });
        }

        targets
    }
}
