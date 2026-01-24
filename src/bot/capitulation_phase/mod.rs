use crate::bot::{ClosedPosition, OpenPosition, Position};
use crate::exchange::bitget::PlaceOrderData;
use crate::exchange::Exchange;
use crate::helper::{
    Helper, PartialProfitTarget, CAPITULATION_PHASE_CLOSED_POSITIONS, CAPITULATION_PHASE_STATE,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use log::{info, warn};
use redis::AsyncCommands;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum CapitulationPhase {
    Trade1,
    Trade2,
    Trade3,
    Trade4,
    Trade5,
    Trade6,
    Trade7,
    Trade8,
    Trade9,
    Trade10,
    Trade11,
    Trade12,
    Trade13,
    Trade14,
    Trade15,
    Trade16,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapitulationState {
    pub current_phase: CapitulationPhase,
    pub current_capital: Decimal,
    pub active_position: Option<OpenPosition>,
    pub partial_profit_targets: Vec<PartialProfitTarget>,
    pub cooldown_until: Option<DateTime<Utc>>,
}

impl CapitulationState {
    pub fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap()
    }

    pub async fn load_state(
        conn: &mut redis::aio::MultiplexedConnection,
    ) -> Result<CapitulationState> {
        let key = CAPITULATION_PHASE_STATE;
        let state_str: String = conn.get(key).await?;
        Ok(serde_json::from_str(&state_str)?)
    }

    pub async fn store_state(
        mut conn: redis::aio::MultiplexedConnection,
        state: CapitulationState,
    ) -> Result<()> {
        let key = CAPITULATION_PHASE_STATE;
        let _: () = conn.set(key, state.as_str()).await?;
        Ok(())
    }
}

impl Default for CapitulationState {
    fn default() -> Self {
        Self {
            current_phase: CapitulationPhase::Trade1,
            current_capital: dec!(200.0), // Start with $200 USDT
            active_position: None,
            partial_profit_targets: Vec::new(),
            cooldown_until: None,
        }
    }
}

#[derive(Debug)]
pub struct CapitulationStrategy {
    pub leverage: Decimal,
}

impl CapitulationStrategy {
    pub fn new() -> Self {
        Self {
            leverage: dec!(35.0),
        }
    }

    pub fn get_trade_params(
        &self,
        phase: CapitulationPhase,
    ) -> Option<(Decimal, Decimal, Decimal)> {
        match phase {
            CapitulationPhase::Trade1 => Some((dec!(108405.0), dec!(109130.0), dec!(107535.0))),
            CapitulationPhase::Trade2 => Some((dec!(107405.0), dec!(108130.0), dec!(104535.0))),
            CapitulationPhase::Trade3 => Some((dec!(104405.0), dec!(105130.0), dec!(100535.0))),
            CapitulationPhase::Trade4 => Some((dec!(100405.0), dec!(101130.0), dec!(98535.0))),
            CapitulationPhase::Trade5 => Some((dec!(98405.0), dec!(99130.0), dec!(96535.0))),
            CapitulationPhase::Trade6 => Some((dec!(96405.0), dec!(97130.0), dec!(92535.0))),
            CapitulationPhase::Trade7 => Some((dec!(94405.0), dec!(95130.0), dec!(92535.0))),
            CapitulationPhase::Trade8 => Some((dec!(92405.0), dec!(93130.0), dec!(90535.0))),
            CapitulationPhase::Trade9 => Some((dec!(90405.0), dec!(91130.0), dec!(88435.0))),
            CapitulationPhase::Trade10 => Some((dec!(88405.0), dec!(89130.0), dec!(86405.0))),
            CapitulationPhase::Trade11 => Some((dec!(86405.0), dec!(87130.0), dec!(84405.0))),
            CapitulationPhase::Trade12 => Some((dec!(84405.0), dec!(85130.0), dec!(82405.0))),
            CapitulationPhase::Trade13 => Some((dec!(82405.0), dec!(83130.0), dec!(80405.0))),
            CapitulationPhase::Trade14 => Some((dec!(80405.0), dec!(81130.0), dec!(78405.0))),
            CapitulationPhase::Trade15 => Some((dec!(78405.0), dec!(79130.0), dec!(76405.0))),
            CapitulationPhase::Trade16 => Some((dec!(76405.0), dec!(77130.0), dec!(74405.0))),
            CapitulationPhase::Complete => None,
        }
    }

    pub async fn run_cycle(
        &self,
        state: &mut CapitulationState,
        price: Decimal,
        exchange: &dyn Exchange,
        redis_conn: &mut redis::aio::MultiplexedConnection,
    ) -> Result<()> {
        if state.current_phase == CapitulationPhase::Complete {
            info!("Capitulation phase complete. No more trades will be taken.");
            return Ok(());
        }

        if let Some(cooldown) = state.cooldown_until {
            if Utc::now() < cooldown {
                info!("Capitulation cooldown active. Waiting for cooldown to expire...");
                return Ok(());
            } else {
                state.cooldown_until = None;
                info!("Capitulation cooldown expired. Resuming...");
            }
        }

        let params = self.get_trade_params(state.current_phase);
        if params.is_none() {
            state.current_phase = CapitulationPhase::Complete;
            info!("Capitulation phase complete. No more trades will be taken.");
            return Ok(());
        }
        let (_, sl, tp) = params.unwrap();

        info!(
            "Capitulation Phase Active Position state: {:?}",
            state.active_position
        );
        match &state.active_position {
            None => {
                // Look for entry in ANY phase
                let all_phases = [
                    // CapitulationPhase::Trade1,
                    // CapitulationPhase::Trade2,
                    // CapitulationPhase::Trade3,
                    // CapitulationPhase::Trade4,
                    // CapitulationPhase::Trade5,
                    // CapitulationPhase::Trade6,
                    // CapitulationPhase::Trade7,
                    // CapitulationPhase::Trade8,
                    // CapitulationPhase::Trade9,
                    CapitulationPhase::Trade10,
                    CapitulationPhase::Trade11,
                    CapitulationPhase::Trade12,
                    CapitulationPhase::Trade13,
                    CapitulationPhase::Trade14,
                    CapitulationPhase::Trade15,
                    CapitulationPhase::Trade16,
                ];

                for phase in all_phases {
                    if let Some((entry, sl, tp)) = self.get_trade_params(phase) {
                        if price <= entry && price > (entry - (price * dec!(0.00075))) {
                            // Small buffer for execution
                            state.current_phase = phase; // Set the detected phase
                            info!(
                                "Capitulation Phase {:?}: Entering SHORT at {}",
                                state.current_phase, price
                            );

                            if state.current_capital <= dec!(60.0) {
                                info!("Capitulation Phase Capital {:?}: Not enough capital to enter trade {}", state.current_phase, state.current_capital);
                                state.current_capital = dec!(200.0);
                            }

                            let quantity = Helper::contract_amount(
                                price,
                                state.current_capital,
                                self.leverage,
                            );

                            let mut open_pos = OpenPosition {
                                id: uuid::Uuid::new_v4(),
                                pos: Position::Short,
                                entry_price: price,
                                position_size: quantity,
                                entry_time: chrono::Utc::now(),
                                tp: Some(tp),
                                sl: Some(sl),
                                margin: Some(state.current_capital),
                                quantity: Some(quantity),
                                leverage: Some(self.leverage),
                                risk_pct: Some(dec!(0.10)),
                                order_id: None,
                            };

                            let exec = exchange.place_market_order(open_pos.clone()).await?;
                            open_pos.order_id = Some(exec.order_id);
                            state.active_position = Some(open_pos.clone());

                            // Build partial profit targets
                            state.partial_profit_targets = Helper::build_profit_targets(
                                price,
                                state.current_capital,
                                self.leverage,
                                dec!(500.00), // Using 2000 as default diff for building targets
                                Position::Short,
                            );
                            info!(
                                "Built {:?} partial profit targets",
                                state.partial_profit_targets
                            );
                            CapitulationState::store_state(redis_conn.clone(), state.clone())
                                .await?;

                            // Break after finding the first valid entry to avoid double execution
                            break;
                        }
                    }
                }
            }
            Some(pos) => {
                // Check SL or TP (Prefer dynamic SL/TP from active position)
                let actual_sl = pos.sl.unwrap_or(sl);
                //let actual_tp = pos.tp.unwrap_or(tp);

                if price >= actual_sl {
                    if price >= actual_sl && state.partial_profit_targets.len() == 4 {
                        warn!(
                        "Capitulation Phase {:?}: STOP LOSS HIT (no partial profits taken) at {} (SL: {})",
                        state.current_phase, price, actual_sl
                    );
                        exchange.modify_market_order(pos.clone()).await?;

                        // Store history
                        let pnl = (pos.entry_price - price) * pos.quantity.unwrap();
                        let closed = ClosedPosition {
                            id: pos.id,
                            position: Some(Position::Short),
                            side: Some(Position::Short),
                            entry_price: pos.entry_price,
                            entry_time: pos.entry_time,
                            exit_price: price,
                            exit_time: Utc::now(),
                            pnl,
                            quantity: pos.quantity,
                            sl: pos.sl,
                            roi: Some((pnl / state.current_capital) * dec!(100.0)),
                            leverage: pos.leverage,
                            margin: pos.margin,
                            order_id: pos.order_id.clone(),
                            pnl_after_fees: None,
                            exit_fee: None,
                        };
                        let _: () = redis_conn
                            .rpush(CAPITULATION_PHASE_CLOSED_POSITIONS, closed.as_str())
                            .await?;

                        state.current_capital += pnl;
                        state.active_position = None;
                        state.partial_profit_targets.clear();
                        state.cooldown_until = Some(Utc::now() + chrono::Duration::minutes(240));
                        warn!("Cooldown active until {:?}", state.cooldown_until);
                        CapitulationState::store_state(redis_conn.clone(), state.clone()).await?;
                    } else {
                        warn!(
                            "Capitulation Phase {:?}: PARTIAL PROFIT STOP LOSS HIT at {} (SL: {})",
                            state.current_phase, price, actual_sl
                        );
                        exchange.modify_market_order(pos.clone()).await?; // Assuming this closes it

                        // Store history
                        let pnl = (pos.entry_price - price) * pos.quantity.unwrap();
                        let closed = ClosedPosition {
                            id: pos.id,
                            position: Some(Position::Short),
                            side: Some(Position::Short),
                            entry_price: pos.entry_price,
                            entry_time: pos.entry_time,
                            exit_price: price,
                            exit_time: Utc::now(),
                            pnl,
                            quantity: pos.quantity,
                            sl: pos.sl,
                            roi: Some((pnl / state.current_capital) * dec!(100.0)),
                            leverage: pos.leverage,
                            margin: pos.margin,
                            order_id: pos.order_id.clone(),
                            pnl_after_fees: None,
                            exit_fee: None,
                        };
                        let _: () = redis_conn
                            .rpush(CAPITULATION_PHASE_CLOSED_POSITIONS, closed.as_str())
                            .await?;

                        state.current_capital += pnl;
                        state.active_position = None;
                        state.partial_profit_targets.clear();
                        state.cooldown_until = None; //Some(Utc::now() + chrono::Duration::minutes(60));
                        CapitulationState::store_state(redis_conn.clone(), state.clone()).await?;
                    }
                } else if price <= tp {
                    info!(
                        "Capitulation Phase {:?}: TAKE PROFIT HIT at {} (TP: {})",
                        state.current_phase, price, tp
                    );

                    // Close position
                    exchange.modify_market_order(pos.clone()).await?;

                    // Simple PNL Calculation for compounding
                    let pnl = (pos.entry_price - price) * pos.quantity.unwrap();
                    state.current_capital += pnl;

                    // Store history
                    let closed = ClosedPosition {
                        id: pos.id,
                        position: Some(Position::Short),
                        side: Some(Position::Short),
                        entry_price: pos.entry_price,
                        entry_time: pos.entry_time,
                        exit_price: price,
                        exit_time: Utc::now(),
                        pnl,
                        quantity: pos.quantity,
                        sl: pos.sl,
                        roi: Some((pnl / state.current_capital) * dec!(100.0)),
                        leverage: pos.leverage,
                        margin: pos.margin,
                        order_id: pos.order_id.clone(),
                        pnl_after_fees: None,
                        exit_fee: None,
                    };
                    let _: () = redis_conn
                        .rpush(CAPITULATION_PHASE_CLOSED_POSITIONS, closed.as_str())
                        .await?;

                    info!("New compounded capital: {}", state.current_capital);

                    state.active_position = None;
                    state.partial_profit_targets.clear();
                    state.current_phase = match state.current_phase {
                        CapitulationPhase::Trade1 => CapitulationPhase::Trade2,
                        CapitulationPhase::Trade2 => CapitulationPhase::Trade3,
                        CapitulationPhase::Trade3 => CapitulationPhase::Trade4,
                        CapitulationPhase::Trade4 => CapitulationPhase::Trade5,
                        CapitulationPhase::Trade5 => CapitulationPhase::Trade6,
                        CapitulationPhase::Trade6 => CapitulationPhase::Trade7,
                        CapitulationPhase::Trade7 => CapitulationPhase::Trade8,
                        CapitulationPhase::Trade8 => CapitulationPhase::Trade9,
                        CapitulationPhase::Trade9 => CapitulationPhase::Trade10,
                        CapitulationPhase::Trade10 => CapitulationPhase::Trade11,
                        CapitulationPhase::Trade11 => CapitulationPhase::Trade12,
                        CapitulationPhase::Trade12 => CapitulationPhase::Trade13,
                        CapitulationPhase::Trade13 => CapitulationPhase::Trade14,
                        CapitulationPhase::Trade14 => CapitulationPhase::Trade15,
                        CapitulationPhase::Trade15 => CapitulationPhase::Trade16,
                        _ => CapitulationPhase::Complete,
                    };
                    CapitulationState::store_state(redis_conn.clone(), state.clone()).await?;
                } else {
                    if state.partial_profit_targets.len() > 0 && state.active_position.is_some() {
                        // Check Partial Profits
                        let mut hit_idx = None;
                        for (i, target) in state.partial_profit_targets.iter().enumerate() {
                            if price <= target.target_price {
                                hit_idx = Some(i);
                                break;
                            }
                        }

                        if let Some(idx) = hit_idx {
                            let target = state.partial_profit_targets.remove(idx);
                            info!("Capitulation Partial Profit Hit: {}", target);

                            let qty_to_close = target.size_btc;
                            let mut modified_pos = pos.clone();
                            modified_pos.quantity = Some(qty_to_close);

                            let _: PlaceOrderData =
                                exchange.modify_market_order(modified_pos).await?;

                            // Calculate realized PNL for this partial target
                            let pnl = (pos.entry_price - target.target_price) * qty_to_close;
                            state.current_capital += pnl;

                            // Store partial closure history
                            let closed = ClosedPosition {
                                id: pos.id,
                                position: Some(Position::Short),
                                side: Some(Position::Short),
                                entry_price: pos.entry_price,
                                entry_time: pos.entry_time,
                                exit_price: target.target_price,
                                exit_time: Utc::now(),
                                pnl,
                                quantity: Some(qty_to_close),
                                sl: pos.sl,
                                roi: Some((pnl / state.current_capital) * dec!(100.0)),
                                leverage: pos.leverage,
                                margin: pos.margin,
                                order_id: pos.order_id.clone(),
                                pnl_after_fees: None,
                                exit_fee: None,
                            };
                            let _: () = redis_conn
                                .rpush(CAPITULATION_PHASE_CLOSED_POSITIONS, closed.as_str())
                                .await?;

                            // Update remaining quantity and size in active position
                            if let Some(active) = &mut state.active_position {
                                if let Some(q) = active.quantity {
                                    let new_qty = q - qty_to_close;
                                    active.quantity = Some(new_qty);
                                    active.position_size = new_qty; // Consistent with entry logic (position_size = quantity)
                                }
                                if let Some(sl_update) = target.sl {
                                    active.sl = Some(sl_update);
                                }
                            }

                            // Persist state
                            CapitulationState::store_state(redis_conn.clone(), state.clone())
                                .await?;
                        }
                    } else {
                        state.active_position = None;
                        state.partial_profit_targets.clear();
                        CapitulationState::store_state(redis_conn.clone(), state.clone()).await?;
                    }
                }
            }
        }

        Ok(())
    }
}
