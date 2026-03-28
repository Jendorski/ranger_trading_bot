use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;

use crate::bot::OpenPosition;
use crate::exchange::bitget::fees::{ExecutionType, VipFeeRate};
use crate::helper::Helper;

#[derive(Debug, Clone)]
pub struct BitunixFuturesFees {
    pub maker_fee: f64,
    pub taker_fee: f64,
}

impl BitunixFuturesFees {
    pub fn new(maker_fee: f64, taker_fee: f64) -> Self {
        Self {
            maker_fee,
            taker_fee,
        }
    }

    pub fn fee_on_notional(&self, price: Decimal, size: Decimal, exec: ExecutionType) -> Decimal {
        let rate = match exec {
            ExecutionType::Maker => self.maker_fee,
            ExecutionType::Taker => self.taker_fee,
        };
        price * size * Decimal::from_f64(rate).unwrap_or_default()
    }

    pub fn calc_margin_for_entry(
        &self,
        entry_price: Decimal,
        position_size: Decimal,
        margin: Decimal,
    ) -> Decimal {
        let entry_fee = self.fee_on_notional(entry_price, position_size, ExecutionType::Taker);
        margin - entry_fee
    }

    pub fn calc_pnl_for_exit(
        &self,
        open_position: &OpenPosition,
        current_price: Decimal,
    ) -> (Decimal, Decimal) {
        let exit_fee = self.fee_on_notional(
            current_price,
            open_position.position_size,
            ExecutionType::Taker,
        );
        let pnl = Helper::compute_pnl(
            open_position.pos,
            open_position.entry_price,
            open_position.position_size,
            current_price,
        );
        (pnl - exit_fee, exit_fee)
    }

    /// Returns a VipFeeRate-shaped struct to satisfy the Exchange trait's get_fee_rates().
    pub fn as_vip_fee_rate(&self) -> VipFeeRate {
        VipFeeRate {
            level: "bitunix".to_string(),
            deal_amount: "0".to_string(),
            asset_amount: "0".to_string(),
            taker_fee_rate: self.taker_fee,
            maker_fee_rate: self.maker_fee,
            btc_withdraw_amount: "0".to_string(),
            usdt_withdraw_amount: "0".to_string(),
        }
    }
}
