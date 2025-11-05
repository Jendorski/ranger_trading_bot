use crate::{bot::Position, config::Config};
use chrono::{Local, Timelike};
use uuid::Uuid;

pub const TRADING_SCALPER_BOT_ACTIVE: &str = "trading_scalper_bot::active";
pub const TRADIN_SCALPER_BOT_POSITION: &str = "trading_scalper_bot::position";
pub const SCALPER_CLOSED_POSITIONS: &str = "scalper_closed_positions";
pub const TRADING_BOT_ZONES: &str = "trading_bot:zones";
pub const TRADING_BOT_POSITION: &str = "trading_bot:position";
pub const TRADING_BOT_ACTIVE: &str = "trading::active";
pub const TRADING_BOT_CLOSE_POSITIONS: &str = "closed_positions";
pub const TRADING_CAPITAL: &str = "trading_capital";
pub const TRADING_PARTIAL_PROFIT_TARGET: &str = "trading_partial_profit_target";

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
}

impl PartialProfitTarget {
    /// Returns true if price lies in the zone
    #[inline]
    pub fn contains(&self, price: f64, pos: Position) -> bool {
        let mut high = self.target_price;
        if pos == Position::Long {
            high = self.target_price + 100.00;
            return price >= self.target_price || price >= high;
        }

        if pos == Position::Short {
            high = self.target_price - 100.00;
            return price <= self.target_price || price <= high;
        }

        return false;
    }
}

/// An action that the bot will send to the exchange.
#[derive(Debug)]
pub enum TradeAction {
    /// Close a part of the position.
    ClosePartial { order_id: Uuid, quantity: f64 },

    /// Move an existing stop‑loss (or create one if none existed).
    MoveStopLoss { new_stop_price: f64 },
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

        if pos == Position::Long && exit_price != 0.00 && entry_price != 0.00 {
            pnl = exit_price - entry_price;
        }

        if pos == Position::Short && exit_price != 0.00 && entry_price != 0.00 {
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
        now.hour() == 00 && now.minute() == 0
    }

    /// Percentage PnL of a single trade
    pub fn pnl_percent(entry: f64, exit: f64, leverage: f64, pos: Position) -> f64 {
        if !entry.is_finite() || !exit.is_finite() {
            return 0.00;
        }

        if entry == 0.00 || exit == 0.00 {
            return 0.00;
        }

        let mut pl = 0.00;

        if pos == Position::Long {
            pl = (exit - entry) / entry;
        }

        if pos == Position::Short {
            pl = (entry - exit) / entry;
        }

        if pos == Position::Flat {
            pl = 0.00;
        }

        return pl * leverage * 100.00;
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

    pub fn calc_price_difference(entry_price: f64, current_price: f64, pos: Position) -> f64 {
        if entry_price.is_finite() && current_price.is_finite() {
            if pos == Position::Long {
                return current_price - entry_price;
            }

            if pos == Position::Short {
                return entry_price - current_price;
            }
        }

        return 0.00;
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

    pub fn compute_partial_profit_target(
        entry_price: f64,
        pos: Position,
    ) -> Vec<PartialProfitTarget> {
        //This should be gotten from the config
        let profit_factor = Self::from_config().config.profit_factor;
        let fraction = 0.25;

        let mut vector: Vec<PartialProfitTarget> = Vec::new();

        let mut count = 0;
        let mut tp = entry_price;

        if pos == Position::Long {
            tp = entry_price + profit_factor;
        }

        if pos == Position::Short {
            tp = entry_price - profit_factor;
        }

        // Loop as long as 'count' is less than 4.
        while count < 4 {
            if pos == Position::Long {
                vector.push(PartialProfitTarget {
                    target_price: tp,
                    fraction,
                });
                tp += profit_factor;
            }

            if pos == Position::Short {
                vector.push(PartialProfitTarget {
                    target_price: tp,
                    fraction,
                });
                tp -= profit_factor;
            }

            // Increment the counter by 1 in each iteration.
            count += 1;
        }

        vector
    }
}
