use anyhow::anyhow;
use anyhow::Result;
use chrono::{DateTime, Utc};
use log::{info, warn};
use redis::{AsyncCommands, RedisError};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::ops::Div;
use std::time::Duration;
use uuid::Uuid;

use crate::bot::zones::ZoneGuard;
use crate::bot::zones::ZoneId;
use crate::bot::zones::{Zone, Zones};
use crate::calendar::MacroGuard;
use crate::config::Config;
use crate::exchange::bitget::fees::BitgetFuturesFees;
use crate::exchange::bitget::BitgetWsClient;
use crate::exchange::bitget::PlaceOrderData;
use crate::exchange::Exchange;
use crate::graph::Graph;
use crate::helper::TRADING_BOT_LOSS_COUNT;
use crate::helper::TRADING_PARTIAL_PROFIT_TARGET;
use crate::helper::{
    Helper, PartialProfitTarget, TRADING_BOT_ACTIVE, TRADING_BOT_CLOSE_POSITIONS,
    TRADING_BOT_POSITION, TRADING_BOT_ZONES, TRADING_CAPITAL,
};
use futures_util::StreamExt;

//pub mod scalper;

pub mod capitulation_phase;
pub mod zones;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Position {
    Flat,
    Long,
    Short,
}

impl Position {
    fn as_str(&self) -> &'static str {
        match self {
            Position::Flat => "Flat",
            Position::Long => "Long",
            Position::Short => "Short",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosedPosition {
    pub id: uuid::Uuid,
    pub position: Option<Position>,
    pub side: Option<Position>,
    pub entry_price: Decimal,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub entry_time: DateTime<Utc>,
    pub exit_price: Decimal,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub exit_time: DateTime<Utc>,
    pub pnl: Decimal,
    pub quantity: Option<Decimal>,
    //pub tp: Option<f64>,
    pub sl: Option<Decimal>,
    pub roi: Option<Decimal>,
    pub leverage: Option<Decimal>,
    pub margin: Option<Decimal>,
    pub order_id: Option<String>,
    pub pnl_after_fees: Option<Decimal>,
    pub exit_fee: Option<Decimal>,
}

impl ClosedPosition {
    pub fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenPosition {
    pub id: Uuid,             // unique identifier
    pub pos: Position,        // Long / Short
    pub entry_price: Decimal, // price at which we entered
    pub position_size: Decimal,
    #[serde(with = "chrono::serde::ts_milliseconds")] // store as epoch ms
    pub entry_time: DateTime<Utc>, // UTC timestamp of entry
    pub tp: Option<Decimal>,
    pub sl: Option<Decimal>,
    pub margin: Option<Decimal>,
    pub quantity: Option<Decimal>,
    pub leverage: Option<Decimal>,
    pub risk_pct: Option<Decimal>,
    pub order_id: Option<String>,
}

impl OpenPosition {
    pub fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap()
    }

    fn default_open_position() -> OpenPosition {
        OpenPosition {
            id: Uuid::nil(),
            pos: Position::Flat,
            entry_price: dec!(0.00),
            entry_time: Utc::now(),
            position_size: dec!(0.015),
            tp: Some(dec!(0.00)),
            sl: Some(dec!(0.00)),
            margin: Some(dec!(50.00)),
            quantity: Some(dec!(0.015)),
            risk_pct: Some(dec!(0.05)),
            leverage: Some(dec!(35.00)),
            order_id: Some("".to_string()),
        }
    }

    async fn load_open_position(
        conn: &mut redis::aio::MultiplexedConnection,
        //id: Uuid,
    ) -> Result<OpenPosition> {
        let key = format!("trading::active",);

        let open_pos: String = conn.get(key).await?;

        Ok(serde_json::from_str(&open_pos)?)
    }

    async fn store_open_position(
        mut conn: redis::aio::MultiplexedConnection,
        open_pos: OpenPosition,
    ) -> Result<()> {
        let key = TRADING_BOT_ACTIVE;

        let _: () = conn.set(key, open_pos.as_str()).await?;

        Ok(())
    }
}

/// Trading state – we keep track of whether we have an open position
#[derive(Debug)]
pub struct Bot<'a> {
    pub open_pos: OpenPosition,

    pub pos: Position,

    pub zones: Zones,

    //pub default_zone: Zone,
    pub loss_count: usize,

    //pub smc: SmcEngine,

    // a *mutable* reference to the redis connection
    redis_conn: redis::aio::MultiplexedConnection,

    config: &'a Config,

    current_margin: Decimal,

    partial_profit_target: Vec<PartialProfitTarget>,

    fees: BitgetFuturesFees,

    zone_guard: ZoneGuard,

    macro_guard: MacroGuard,

    pub capitulation_state: capitulation_phase::CapitulationState,
    pub capitulation_strategy: capitulation_phase::CapitulationStrategy,
}

impl<'a> Bot<'a> {
    pub async fn new(
        mut conn: redis::aio::MultiplexedConnection,
        config: &'a Config,
    ) -> Result<Self> {
        let pos: Position = Self::load_position(&mut conn)
            .await
            .unwrap_or_else(|_| Position::Flat);

        let zones: Zones = Self::load_zones(&mut conn)
            .await
            .unwrap_or_else(|_| Zones::default());

        let open_pos = OpenPosition::load_open_position(&mut conn)
            .await
            .unwrap_or_else(|_| OpenPosition::default_open_position());

        let current_margin = Self::load_current_margin(&mut conn, config).await;

        let partial_profit_target = Self::load_partial_profit_target(&mut conn)
            .await
            .unwrap_or_else(|_| [].to_vec());

        let loss_count = Self::load_loss_count(&mut conn).await.unwrap_or_else(|_| 0);

        //let smc = SmcEngine::new(3, 3);

        let fees = BitgetFuturesFees::new(conn.clone());

        let zone_guard = ZoneGuard::new(2, conn.clone(), 60 * 60 * 8);

        let macro_guard = MacroGuard::new(&mut conn.clone()).await?;

        let capitulation_state = capitulation_phase::CapitulationState::load_state(&mut conn)
            .await
            .unwrap_or_else(|_| capitulation_phase::CapitulationState::default());
        let capitulation_strategy = capitulation_phase::CapitulationStrategy::new();

        Ok(Self {
            open_pos,
            pos,
            zones,
            loss_count,
            redis_conn: conn,
            config,
            current_margin,
            partial_profit_target,
            fees,
            zone_guard,
            macro_guard,
            capitulation_state,
            capitulation_strategy,
        })
    }

    async fn load_loss_count(conn: &mut redis::aio::MultiplexedConnection) -> Result<usize> {
        let opt: Option<String> = conn.get(TRADING_BOT_LOSS_COUNT).await?;

        let u = serde_json::from_str::<usize>(&opt.unwrap_or("0".to_string()));
        Ok(u.unwrap_or_else(|_| 0))
    }

    async fn load_partial_profit_target(
        conn: &mut redis::aio::MultiplexedConnection,
    ) -> Result<Vec<PartialProfitTarget>> {
        let raw_jsons: String = conn.get(TRADING_PARTIAL_PROFIT_TARGET).await?;

        let vecs = serde_json::from_str::<Vec<PartialProfitTarget>>(&raw_jsons)
            .map_err(|e| anyhow!("Failed to parse: {}", e))?;

        Ok(vecs)
    }

    async fn load_zones(conn: &mut redis::aio::MultiplexedConnection) -> Result<Zones> {
        let json: String = conn.get(TRADING_BOT_ZONES).await?;
        Ok(serde_json::from_str(&json)?)
    }

    pub async fn load_position(conn: &mut redis::aio::MultiplexedConnection) -> Result<Position> {
        let opt: Option<String> = conn.get(TRADING_BOT_POSITION).await?;

        Ok(match opt.as_deref() {
            Some("Flat") => Position::Flat,
            Some("Long") => Position::Long,
            Some("Short") => Position::Short,
            _ => Position::Flat,
        })
    }

    async fn store_position(&mut self, pos: Position, open_pos: OpenPosition) -> Result<()> {
        let _: () = self
            .redis_conn
            .set(TRADING_BOT_POSITION, pos.as_str())
            .await?;

        OpenPosition::store_open_position(self.redis_conn.clone(), open_pos).await?;

        Ok(())
    }

    /// Store *one* closed position in the list named `TRADING_BOT_CLOSE_POSITIONS`.
    pub async fn store_closed_position(
        conn: &mut redis::aio::MultiplexedConnection,
        pos: &ClosedPosition,
    ) -> Result<()> {
        let key = TRADING_BOT_CLOSE_POSITIONS;
        let json = serde_json::to_string(pos)?;

        // LPUSH pushes to the **left** of the list – newest element first
        let _: () = conn.lpush(key, json).await?;

        // OPTIONAL: keep only the last N trades (e.g. 10 000)
        // conn.ltrim(key, 0, 9999).await?;

        Ok(())
    }

    async fn prepare_open_position(
        &mut self,
        pos: Position,
        entry_price: Decimal,
        leverage: Decimal,
        risk_pct: Decimal,
        funding_multiplier: Decimal,
    ) -> OpenPosition {
        let current_margin = self.current_margin * funding_multiplier;

        let sl = Helper::stop_loss_price(entry_price, current_margin, leverage, risk_pct, pos);
        let qty = Helper::contract_amount(entry_price, current_margin, leverage);
        let tp = self
            .partial_profit_target
            .last()
            .unwrap_or(&PartialProfitTarget {
                target_price: dec!(1.11),
                fraction: dec!(0.0),
                sl: Some(dec!(1.11)),
                size_btc: dec!(0.00),
            })
            .target_price;

        let margin_minus_fees = self
            .fees
            .calc_margin_for_entry(entry_price, qty, current_margin)
            .await;
        OpenPosition {
            id: Uuid::new_v4(),
            pos: pos,
            entry_price: entry_price,
            position_size: qty, //does the same thing as quantity :(
            entry_time: Utc::now(),
            tp: Some(tp),
            sl: Some(sl),
            margin: Some(margin_minus_fees),
            quantity: Some(qty),
            leverage: Some(leverage),
            risk_pct: Some(risk_pct),
            order_id: Some("".to_string()),
        }
    }

    async fn delete_partial_profit_target(&mut self) -> Result<()> {
        let _: () = self.redis_conn.del(TRADING_PARTIAL_PROFIT_TARGET).await?;

        self.partial_profit_target = [].to_vec();

        Ok(())
    }

    pub async fn close_long_position(&mut self, price: Decimal) -> Result<()> {
        let dec_config_margin = Helper::f64_to_decimal(self.config.margin);
        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos.margin.unwrap_or(dec_config_margin),
            self.open_pos.entry_price,
            self.pos,
            self.open_pos.position_size,
            price,
        );
        let pnl = Helper::compute_pnl(
            self.pos,
            self.open_pos.entry_price,
            self.open_pos.position_size,
            price,
        );

        let (pnl_after_fees, exit_fee) = self
            .fees
            .calc_pnl_for_exit(self.open_pos.clone(), price)
            .await;
        let closed_pos = ClosedPosition {
            id: self.open_pos.id,
            entry_price: self.open_pos.entry_price,
            exit_price: price,
            exit_time: Utc::now(),
            position: Some(Position::Long),
            side: None,
            entry_time: self.open_pos.entry_time,
            pnl,
            quantity: Some(self.open_pos.position_size),
            sl: self.open_pos.sl,
            roi: Some(roi),
            leverage: self.open_pos.leverage,
            margin: self.open_pos.margin,
            order_id: self.open_pos.order_id.clone(),
            pnl_after_fees: Some(pnl_after_fees),
            exit_fee: Some(exit_fee),
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;

        //update the margin based on the pnl
        let _ = Self::prepare_current_margin(self, pnl_after_fees).await;

        //Track loss count
        let total_profit_count = 4;
        //This means that we did not hit any of the targets
        if self.partial_profit_target.len() == total_profit_count {
            //Track also the zone where we got a loss
            let zone = self
                .zones
                .long_zones
                .iter()
                .find(|z| z.contains(Helper::decimal_to_f64(self.open_pos.entry_price)));

            if !zone.is_none() {
                warn!(
                    "Losing zone found for price: {}; zone: {:?}",
                    self.open_pos.entry_price,
                    zone.unwrap()
                );
                let zone_id = ZoneId::from_zone(zone.unwrap());
                self.zone_guard
                    .record_trade_result(zone_id, Helper::decimal_to_f64(pnl_after_fees))
                    .await;
            }

            info!("Loss count: {}", self.loss_count);
            let _ = self.store_loss_count(pnl_after_fees).await;
        }
        self.loss_count = Self::load_loss_count(&mut self.redis_conn).await?;
        Ok(())
    }

    async fn store_loss_count(&mut self, pnl: Decimal) -> Result<()> {
        if pnl.is_sign_negative() || pnl < dec!(0.00) {
            self.loss_count += 1;

            //Store the loss count in redis for 12hours
            if let Err(e) = self
                .redis_conn
                .set_ex::<_, _, ()>(TRADING_BOT_LOSS_COUNT, self.loss_count, 43200) //12hours reset
                .await
            {
                warn!("Failed to store loss count: {}", e);
            }
        }
        Ok(())
    }

    pub async fn load_current_margin(
        redis_conn: &mut redis::aio::MultiplexedConnection,
        config: &'a Config,
    ) -> Decimal {
        let key = TRADING_CAPITAL;

        let raw_margin: Result<Option<String>, RedisError> = redis_conn.get(key).await;

        let mut margin = match raw_margin {
            Ok(Some(raw_margin)) => serde_json::from_str::<Decimal>(&raw_margin)
                .unwrap_or_else(|_| Helper::f64_to_decimal(config.margin)),
            Ok(None) => Helper::f64_to_decimal(config.margin),
            Err(_) => Helper::f64_to_decimal(config.margin),
        };

        if margin <= dec!(5.00) {
            warn!("margin as we know it, is rekt, {:2}", margin);
            margin = Helper::f64_to_decimal(config.margin);
            return margin;
        }

        return margin;
    }

    pub async fn prepare_current_margin(&mut self, pnl: Decimal) -> Decimal {
        let mut current_margin = Self::load_current_margin(&mut self.redis_conn, self.config).await;
        info!("redis_current_margin: {:?}", current_margin);
        info!("prepare_current_margin pnl: {:?}", pnl);

        current_margin += pnl;
        info!("current_margin, {:2}", current_margin);

        if current_margin <= dec!(5.00) {
            warn!("current_margin is rekt, {:2}", current_margin);
            current_margin = Helper::f64_to_decimal(self.config.margin);
            self.open_pos.margin = Some(current_margin);
        }

        self.current_margin = current_margin;

        let _ = Self::store_current_margin(current_margin, &mut self.redis_conn).await;
        let _ =
            OpenPosition::store_open_position(self.redis_conn.clone(), self.open_pos.clone()).await;

        return current_margin;
    }

    async fn store_current_margin(
        current_margin: Decimal,
        conn: &mut redis::aio::MultiplexedConnection,
    ) -> Result<()> {
        let json = serde_json::to_string(&current_margin).expect("Failed to serialize margin");

        let _: () = conn.set(TRADING_CAPITAL, json).await?;

        Ok(())
    }

    pub async fn close_short_position(&mut self, price: Decimal) -> Result<()> {
        let pnl = Helper::compute_pnl(
            self.open_pos.pos,
            self.open_pos.entry_price,
            self.open_pos.position_size,
            price,
        );
        let (pnl_after_fees, exit_fee) = self
            .fees
            .calc_pnl_for_exit(self.open_pos.clone(), price)
            .await;
        info!(
            "close_short_position: pnl, pnl_after_fees, exit_fees -> {:?}, {:?}, {:?}",
            pnl, pnl_after_fees, exit_fee
        );
        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos
                .margin
                .unwrap_or(Helper::f64_to_decimal(self.config.margin)),
            self.open_pos.entry_price,
            self.open_pos.pos,
            self.open_pos.position_size,
            price,
        );
        let closed_pos = ClosedPosition {
            id: self.open_pos.id,
            entry_price: self.open_pos.entry_price,
            exit_price: price,
            exit_time: Utc::now(),
            position: Some(Position::Short),
            side: Some(Position::Short),
            entry_time: self.open_pos.entry_time,
            pnl,
            quantity: Some(self.open_pos.position_size),
            sl: self.open_pos.sl,
            roi: Some(roi),
            leverage: self.open_pos.leverage,
            margin: self.open_pos.margin,
            order_id: self.open_pos.order_id.clone(),
            pnl_after_fees: Some(pnl_after_fees),
            exit_fee: Some(exit_fee),
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;

        //update the margin based on the pnl
        let _ = Self::prepare_current_margin(self, pnl_after_fees).await;

        //Track loss count
        let total_profit_count = 4;
        //This means that we did not hit any of the targets
        if self.partial_profit_target.len() == total_profit_count {
            //Track also the zone where we got a loss
            let zone = self
                .zones
                .short_zones
                .iter()
                .find(|z| z.contains(Helper::decimal_to_f64(self.open_pos.entry_price)));

            if !zone.is_none() {
                warn!(
                    "Losing zone found for price: {}; zone: {:?}",
                    self.open_pos.entry_price,
                    zone.unwrap()
                );
                let zone_id = ZoneId::from_zone(zone.unwrap());
                self.zone_guard
                    .record_trade_result(zone_id, Helper::decimal_to_f64(pnl_after_fees))
                    .await;
            }

            let _ = self.store_loss_count(pnl_after_fees).await;
        }

        self.loss_count = Self::load_loss_count(&mut self.redis_conn).await?;

        Ok(())
    }

    pub async fn take_profit_on_long(
        &mut self,
        price: Decimal,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        info!("Ranger Taking profit on LONG at {:.2}", price);

        self.open_pos.tp = Some(price);

        let exec_price: PlaceOrderData =
            exchange.modify_market_order(self.open_pos.clone()).await?;

        info!("Ranger Closed LONG at {:?}", exec_price);

        let _: () = Self::close_long_position(self, price).await?;

        self.pos = Position::Flat;

        Ok(())
    }

    async fn take_partial_profit_on_long(
        &mut self,
        price: f64,
        target: PartialProfitTarget,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        let mut remaining_size = self.open_pos.quantity.unwrap_or_default();

        let qty_to_close = target.size_btc;

        let dec_price = Helper::f64_to_decimal(price);

        if qty_to_close <= dec!(0.0000) {
            let _: () = Self::close_long_position(self, dec_price).await?;
        }

        if self.partial_profit_target.len() == 0 {
            info!(
                "ALL TARGETS HIT FOR LONG!: {:?}",
                self.partial_profit_target
            );
            self.pos = Position::Flat;
            self.partial_profit_target = [].to_vec();
        }

        remaining_size -= qty_to_close;

        if remaining_size <= dec!(0.0000) {
            self.open_pos.quantity = Some(remaining_size);
            self.open_pos.position_size = remaining_size;
            let _: () = Self::close_long_position(self, dec_price).await?;
        }

        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos
                .margin
                .unwrap_or(Helper::f64_to_decimal(self.config.margin)),
            self.open_pos.entry_price,
            self.pos,
            qty_to_close,
            dec_price,
        );

        let pnl = Helper::compute_pnl(self.pos, self.open_pos.entry_price, qty_to_close, dec_price);

        let modified_open_pos = OpenPosition {
            id: self.open_pos.id,
            pos: self.open_pos.pos,
            entry_price: self.open_pos.entry_price,
            position_size: qty_to_close,
            entry_time: self.open_pos.entry_time,
            tp: self.open_pos.tp,
            sl: self.open_pos.sl,
            margin: self.open_pos.margin,
            quantity: Some(qty_to_close),
            leverage: self.open_pos.leverage,
            risk_pct: self.open_pos.risk_pct,
            order_id: self.open_pos.order_id.clone(),
        };

        let (pnl_after_fees, exit_fee) = self
            .fees
            .calc_pnl_for_exit(modified_open_pos.clone(), dec_price)
            .await;

        //Exchange call to take profit
        //self.open_pos.tp = Some(dec_price);
        let exec_price: PlaceOrderData = exchange
            .modify_market_order(modified_open_pos.clone())
            .await?;
        info!("exec_price: {:?}", exec_price);

        let closed_pos = ClosedPosition {
            id: self.open_pos.id,
            entry_price: self.open_pos.entry_price,
            exit_price: dec_price,
            exit_time: Utc::now(),
            position: Some(Position::Long),
            side: Some(Position::Long),
            entry_time: self.open_pos.entry_time,
            pnl,
            quantity: Some(qty_to_close),
            sl: self.open_pos.sl,
            roi: Some(roi),
            leverage: self.open_pos.leverage,
            margin: self.open_pos.margin,
            order_id: self.open_pos.order_id.clone(),
            pnl_after_fees: Some(pnl_after_fees),
            exit_fee: Some(exit_fee),
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;

        //update the margin based on the pnl
        let _ = Self::prepare_current_margin(self, pnl_after_fees).await;

        self.open_pos = OpenPosition {
            id: self.open_pos.id,
            pos: self.open_pos.pos,
            entry_price: self.open_pos.entry_price,
            position_size: remaining_size,
            entry_time: self.open_pos.entry_time,
            tp: Some(target.target_price),
            sl: target.sl,
            margin: self.open_pos.margin,
            quantity: Some(remaining_size),
            leverage: self.open_pos.leverage,
            risk_pct: self.open_pos.risk_pct,
            order_id: Some(exec_price.order_id),
        };

        warn!("NEW SL for LONG is: {:?}", target.sl);
        self.store_position(self.pos, self.open_pos.clone()).await?;
        Ok(())
    }

    async fn take_partial_profit_on_short(
        &mut self,
        price: f64,
        target: PartialProfitTarget,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        let mut remaining_size = self.open_pos.quantity.unwrap_or_default();
        let qty_to_close = target.size_btc;
        let dec_price = Helper::f64_to_decimal(price);

        if qty_to_close <= dec!(0.0000) {
            let _: () = Self::close_short_position(self, dec_price).await?;
        }

        if self.partial_profit_target.len() == 0 {
            info!(
                "ALL TARGETS HIT FOR SHORT!: {:?}",
                self.partial_profit_target
            );
            self.pos = Position::Flat;
            self.partial_profit_target = [].to_vec();
        }

        remaining_size -= qty_to_close;

        if remaining_size <= dec!(0.0000) {
            self.open_pos.quantity = Some(remaining_size);
            self.open_pos.position_size = remaining_size;
            let _: () = Self::close_short_position(self, dec_price).await?;
        }

        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos
                .margin
                .unwrap_or(Helper::f64_to_decimal(self.config.margin)),
            self.open_pos.entry_price,
            self.pos,
            qty_to_close,
            dec_price,
        );

        let pnl = Helper::compute_pnl(self.pos, self.open_pos.entry_price, qty_to_close, dec_price);

        let modified_open_pos = OpenPosition {
            id: self.open_pos.id,
            pos: self.open_pos.pos,
            entry_price: self.open_pos.entry_price,
            position_size: qty_to_close,
            entry_time: self.open_pos.entry_time,
            tp: self.open_pos.tp,
            sl: self.open_pos.sl,
            margin: self.open_pos.margin,
            quantity: Some(qty_to_close),
            leverage: self.open_pos.leverage,
            risk_pct: self.open_pos.risk_pct,
            order_id: self.open_pos.order_id.clone(),
        };

        let (pnl_after_fees, exit_fee) = self
            .fees
            .calc_pnl_for_exit(modified_open_pos.clone(), dec_price)
            .await;

        //Exchange call to take profit
        //self.open_pos.tp = Some(dec_price);
        let exec_price: PlaceOrderData = exchange
            .modify_market_order(modified_open_pos.clone())
            .await?;

        let closed_pos = ClosedPosition {
            id: self.open_pos.id,
            entry_price: self.open_pos.entry_price,
            exit_price: dec_price,
            exit_time: Utc::now(),
            position: Some(Position::Short),
            side: Some(Position::Short),
            entry_time: self.open_pos.entry_time,
            pnl,
            quantity: Some(qty_to_close),
            sl: self.open_pos.sl,
            roi: Some(roi),
            leverage: self.open_pos.leverage,
            margin: self.open_pos.margin,
            order_id: Some(exec_price.order_id),
            pnl_after_fees: Some(pnl_after_fees),
            exit_fee: Some(exit_fee),
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;

        //update the margin based on the pnl
        let _ = Self::prepare_current_margin(self, pnl_after_fees).await;

        self.open_pos = OpenPosition {
            id: self.open_pos.id,
            pos: self.open_pos.pos,
            entry_price: self.open_pos.entry_price,
            position_size: remaining_size,
            entry_time: self.open_pos.entry_time,
            tp: Some(target.target_price),
            sl: target.sl,
            margin: self.open_pos.margin,
            quantity: Some(remaining_size),
            leverage: self.open_pos.leverage,
            risk_pct: self.open_pos.risk_pct,
            order_id: self.open_pos.order_id.clone(),
        };
        self.store_position(self.pos, self.open_pos.clone()).await?;

        warn!("NEW SL for SHORT is: {:?}", target.sl);

        Ok(())
    }

    //This takes FULL profit on a short position
    pub async fn take_profit_on_short(
        &mut self,
        price: f64,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        info!("Ranger Covering SHORT at {:.2}", price);
        let dec_price = Helper::f64_to_decimal(price);

        self.open_pos.tp = Some(dec_price);

        let exec_price: PlaceOrderData =
            exchange.modify_market_order(self.open_pos.clone()).await?;

        info!("Ranger Covered SHORT at {:?}", exec_price);

        let _: () = Self::close_short_position(self, dec_price).await?;

        self.pos = Position::Flat;

        Ok(())
    }

    fn determine_profit_difference(&mut self, entry_price: f64, pos: Position) -> f64 {
        if pos == Position::Long {
            // Filter zones above entry price (for LONG TP)
            let valid_zones: Vec<_> = self
                .zones
                .short_zones
                .iter()
                .filter(|zone| zone.low > entry_price)
                .collect();

            if valid_zones.is_empty() {
                return 0.00;
            }

            // Find the nearest zone by comparing distance to zone low
            let the_zone = *valid_zones
                .into_iter()
                .min_by(|a, b| {
                    let dist_a = (a.low - entry_price).abs();
                    let dist_b = (b.low - entry_price).abs();
                    dist_a.partial_cmp(&dist_b).unwrap()
                })
                .unwrap_or(&Zone {
                    low: 0.00,
                    high: 0.00,
                    side: zones::Side::Short,
                });

            return the_zone.low - entry_price;
        }

        if pos == Position::Short {
            // Filter zones below entry price (for SHORT TP)
            let valid_zones: Vec<_> = self
                .zones
                .long_zones
                .iter()
                .filter(|zone| zone.high < entry_price)
                .collect();

            if valid_zones.is_empty() {
                return 0.00;
            }

            // Find the nearest zone by comparing distance to zone high
            let the_zone = *valid_zones
                .into_iter()
                .min_by(|a, b| {
                    let dist_a = (entry_price - a.high).abs();
                    let dist_b = (entry_price - b.high).abs();
                    dist_a.partial_cmp(&dist_b).unwrap()
                })
                .unwrap_or(&Zone {
                    low: 0.00,
                    high: 0.00,
                    side: zones::Side::Long,
                });

            return entry_price - the_zone.high;
        }

        return 0.00;
    }

    async fn store_partial_profit_targets(
        &mut self,
        entry_price: f64,
        pos: Position,
    ) -> Result<()> {
        self.zones = Bot::load_zones(&mut self.redis_conn)
            .await
            .unwrap_or(Zones::default());

        let price_difference = Self::determine_profit_difference(self, entry_price, pos);

        let profit_count = 4.00;
        let mut ranger_price_difference = self.config.ranger_price_difference;
        if price_difference.is_finite() && price_difference != 0.00 {
            ranger_price_difference = price_difference.div(profit_count);
        }

        let current_margin = self.current_margin;

        let dec_entry_price = Decimal::from_f64(entry_price).unwrap();
        let dec_leverage = Decimal::from_f64(self.config.leverage).unwrap();
        let dec_ranger_price_difference = Decimal::from_f64(ranger_price_difference).unwrap();

        let ppt = Helper::build_profit_targets(
            dec_entry_price,
            current_margin,
            dec_leverage,
            dec_ranger_price_difference,
            pos,
        );

        self.partial_profit_target = ppt.clone();

        let _: () = self
            .redis_conn
            .set(
                TRADING_PARTIAL_PROFIT_TARGET,
                serde_json::to_string(&ppt.clone()).unwrap(),
            )
            .await?;

        Ok(())
    }

    async fn evaluate_long_partial_profit(
        &mut self,
        price: f64,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        if self.partial_profit_target.len() == 0 {
            info!(
                "ALL TARGETS HIT FOR LONG!: {:?}",
                self.partial_profit_target
            );
            self.pos = Position::Flat;
        }

        let dec_price = Decimal::from_f64(price).unwrap();

        let idx_opt = self
            .partial_profit_target
            .iter()
            .position(|t| dec_price >= t.target_price);

        let idx = idx_opt.unwrap_or(usize::MAX);

        if idx == usize::MAX {
            return Ok(());
        }

        let target = self.partial_profit_target[idx].clone();

        if target.target_price.is_zero() || target.target_price.is_sign_negative() {
            return Ok(());
        }

        info!(
            "LONG: Taking Partial Profits here.... {:?}, Take profit targets: {:?}",
            price, self.partial_profit_target
        );
        let _: () = Self::take_partial_profit_on_long(self, price, target, exchange).await?;

        self.partial_profit_target.remove(idx);

        warn!(
            "self.partial_profit_target: {:?}",
            self.partial_profit_target
        );

        let _: () = self
            .redis_conn
            .set(
                TRADING_PARTIAL_PROFIT_TARGET,
                serde_json::to_string(&self.partial_profit_target.clone()).unwrap(),
            )
            .await?;

        Ok(())
    }

    async fn evaluate_short_partial_profit(
        &mut self,
        price: f64,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        if self.partial_profit_target.len() == 0 {
            info!(
                "ALL TARGETS HIT FOR SHORT!: {:?}",
                self.partial_profit_target
            );
            self.pos = Position::Flat;
        }

        let dec_price = Decimal::from_f64(price).unwrap();

        let idx_opt = self
            .partial_profit_target
            .iter()
            .position(|t| dec_price <= t.target_price);

        let idx = idx_opt.unwrap_or(usize::MAX);

        if idx == usize::MAX {
            return Ok(());
        }

        let target = self.partial_profit_target[idx].clone();
        info!("target: {:?}", target);

        if target.target_price.is_zero() || target.target_price.is_sign_negative() {
            return Ok(());
        }

        info!(
            "SHORT: Taking Partial Profits here.... {:?}, Take profit targets: {:?}",
            price, self.partial_profit_target
        );
        let _: () = Self::take_partial_profit_on_short(self, price, target, exchange).await?;

        self.partial_profit_target.remove(idx);
        warn!(
            "self.partial_profit_target: {:?}",
            self.partial_profit_target
        );

        let _: () = self
            .redis_conn
            .set(
                TRADING_PARTIAL_PROFIT_TARGET,
                serde_json::to_string(&self.partial_profit_target.clone()).unwrap(),
            )
            .await?;

        Ok(())
    }

    pub async fn test(&mut self, exchange: &dyn Exchange) -> Result<()> {
        Ok(())
    }

    async fn run_cycle(&mut self, price: f64, exchange: &dyn Exchange) -> Result<()> {
        let dec_price = Decimal::from_f64(price).unwrap();
        if price == 1.11 {
            warn!("Price failure! -> {:?}", price);
            return Ok(());
        }

        if !self.macro_guard.allow_entry(Utc::now()) {
            warn!("Macro guard not allowing entry");
            return Ok(());
        }

        if self.loss_count >= 2 {
            warn!("Loss count reached 2, skipping cycle");
            self.loss_count = Self::load_loss_count(&mut self.redis_conn).await?;
            info!("loaded loss count: {}", self.loss_count);
            return Ok(());
        }

        //Load the zones, because it's usually updated, periodically.
        self.zones = Bot::load_zones(&mut self.redis_conn)
            .await
            .unwrap_or(Zones::default());

        warn!("Ranger State = {:?}", self.pos);

        match self.pos {
            Position::Flat => {
                if let Some(zone) = self
                    .zones
                    .long_zones
                    .iter()
                    .find(|z| price != 1.11 && z.contains(price))
                {
                    let zone_id = ZoneId::from_zone(zone);
                    let z_guard_trade_result = self.zone_guard.get_trade_result(zone_id).await;
                    if z_guard_trade_result.disabled {
                        warn!("Zone {:?} is not open for trading", zone);
                        return Ok(());
                    }

                    info!("Ranger Entering LONG at {:.2} in zone {:?}", price, zone);
                    let _: () = Self::delete_partial_profit_target(self).await?;

                    self.pos = Position::Long;

                    let funding_rate = exchange.get_funding_rate().await.unwrap_or(0.0);
                    let funding_multiplier = Helper::funding_multiplier(funding_rate, self.pos);
                    info!(
                        "Funding-aware sizing: rate={:.6}, multiplier={:.2}",
                        funding_rate, funding_multiplier
                    );

                    let _: Result<()> =
                        Self::store_partial_profit_targets(self, price, self.pos).await;

                    self.open_pos = Self::prepare_open_position(
                        self,
                        self.pos,
                        dec_price,
                        Helper::f64_to_decimal(self.config.leverage),
                        Helper::f64_to_decimal(self.config.ranger_risk_pct),
                        funding_multiplier,
                    )
                    .await;

                    let exec_price: PlaceOrderData =
                        exchange.place_market_order(self.open_pos.clone()).await?;
                    info!("Ranger Long executed at {:?}", exec_price);

                    if exec_price.client_oid == "Failed to place order" {
                        warn!("Failed to place order");
                        //return Ok(());
                    }

                    self.open_pos.order_id = Some(exec_price.order_id);
                } else if let Some(zone) = self
                    .zones
                    .short_zones
                    .iter()
                    .find(|z| price != 1.11 && z.contains(price))
                {
                    let zone_id = ZoneId::from_zone(zone);

                    let z_guard_trade_result = self.zone_guard.get_trade_result(zone_id).await;

                    if z_guard_trade_result.disabled {
                        warn!("{:?} is not open for trading", zone);
                        return Ok(());
                    }

                    info!("Ranger Entering SHORT at {:.2} in zone {:?}", price, zone);
                    let _: () = Self::delete_partial_profit_target(self).await?;

                    self.pos = Position::Short;

                    let funding_rate = exchange.get_funding_rate().await.unwrap_or(0.0);
                    let funding_multiplier = Helper::funding_multiplier(funding_rate, self.pos);
                    info!(
                        "Funding-aware sizing: rate={:.6}, multiplier={:.2}",
                        funding_rate, funding_multiplier
                    );

                    let _: Result<()> =
                        Self::store_partial_profit_targets(self, price, self.pos).await;

                    self.open_pos = Self::prepare_open_position(
                        self,
                        Position::Short,
                        dec_price,
                        Helper::f64_to_decimal(self.config.leverage),
                        Helper::f64_to_decimal(self.config.ranger_risk_pct),
                        funding_multiplier,
                    )
                    .await;

                    let exec_price: PlaceOrderData =
                        exchange.place_market_order(self.open_pos.clone()).await?;
                    info!("Ranger Short executed at {:?}", exec_price);

                    if exec_price.client_oid == "Failed to place order" {
                        warn!("Failed to place order");
                        //return Ok(());
                    }
                    self.open_pos.order_id = Some(exec_price.order_id);
                } else {
                    //Track for new zone targets
                    warn!("Price {:.2} out of any Ranger zone -- staying flat", price);
                }
            }

            Position::Long => {
                //Trigger SL if it's met
                let in_sl = Helper::stop_loss_price(
                    self.open_pos.entry_price,
                    Helper::f64_to_decimal(self.config.margin),
                    Helper::f64_to_decimal(self.config.leverage),
                    Helper::f64_to_decimal(self.config.risk_pct),
                    Position::Long,
                );
                let ssl_hit =
                    Helper::ssl_hit(dec_price, self.pos, self.open_pos.sl.unwrap_or(in_sl));

                if ssl_hit {
                    let _: () = Self::close_long_position(self, dec_price).await?;

                    warn!(
                        "SL for Ranger Long Position entered at {:2}, with SL triggered at {:2}",
                        self.open_pos.entry_price, price
                    );

                    self.pos = Position::Flat;
                }

                // 2️⃣ Take‑profit: exit long when we hit the short zone.
                if self.zones.short_zones.iter().any(|z| z.contains(price)) {
                    Self::take_profit_on_long(self, dec_price, exchange).await?;
                }

                //Take partial profit if we hit a target
                if self.partial_profit_target.len() > 0
                    && self
                        .partial_profit_target
                        .iter()
                        .any(|p| dec_price >= p.target_price)
                {
                    let _ = Self::evaluate_long_partial_profit(self, price, exchange).await;
                }
            }

            Position::Short => {
                //Trigger SL if it's met
                let in_sl = Helper::stop_loss_price(
                    self.open_pos.entry_price,
                    Helper::f64_to_decimal(self.config.margin),
                    Helper::f64_to_decimal(self.config.leverage),
                    Helper::f64_to_decimal(self.config.risk_pct),
                    Position::Short,
                );
                let ssl_hit =
                    Helper::ssl_hit(dec_price, self.pos, self.open_pos.sl.unwrap_or(in_sl));

                if ssl_hit {
                    let _: () = Self::close_short_position(self, dec_price).await?;

                    warn!(
                        "SL for Ranger Short Position entered at {:2}, with SL triggered at {:2}",
                        self.open_pos.entry_price, price
                    );

                    self.pos = Position::Flat;
                }

                // 3️⃣ Cover: exit short when we hit the long zone.
                if self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    Self::take_profit_on_short(self, price, exchange).await?;
                }

                //Take partial profit if we hit a target
                if self.partial_profit_target.len() > 0
                    && self
                        .partial_profit_target
                        .iter()
                        .any(|p| dec_price <= p.target_price)
                {
                    let _ = Self::evaluate_short_partial_profit(self, price, exchange).await;
                }
            }
        }
        self.store_position(self.pos, self.open_pos.clone()).await?;
        Ok(())
    }

    async fn run_capitulation_cycle(&mut self, price: f64, exchange: &dyn Exchange) -> Result<()> {
        let dec_price = Decimal::from_f64(price).unwrap();

        // --- Capitulation Phase Strategy ---
        if let Err(e) = self
            .capitulation_strategy
            .run_cycle(
                &mut self.capitulation_state,
                dec_price,
                exchange,
                &mut self.redis_conn,
            )
            .await
        {
            log::error!("Capitulation strategy error: {}", e);
        }

        // Persist Capitulation State
        let _ = capitulation_phase::CapitulationState::store_state(
            self.redis_conn.clone(),
            self.capitulation_state.clone(),
        )
        .await;

        Ok(())
    }

    pub async fn start_live_trading(&mut self, exchange: &dyn Exchange) -> Result<()> {
        let mut backoff_secs = 1;
        let max_backoff = 64;

        loop {
            info!("Connecting to Ranger live trading via WebSocket...");

            let ticker_stream_result =
                BitgetWsClient::subscribe_tickers("USDT-FUTURES", "BTCUSDT").await;

            match ticker_stream_result {
                std::result::Result::Ok(mut ticker_stream) => {
                    info!("Successfully connected to Bitget WebSocket");
                    backoff_secs = 1; // Reset backoff on success

                    let mut graph = Graph::new();
                    let mut last_midnight_check = Utc::now();

                    while let Some(msg) = ticker_stream.next().await {
                        match msg {
                            std::result::Result::Ok(ticker) => {
                                let price: f64 = ticker.last_pr.parse().unwrap_or(0.0);

                                if price > 0.0 {
                                    info!("Ticker Price = {:.2}", price);

                                    // Run Capitulation Strategy Independently
                                    if let Err(e) =
                                        self.run_capitulation_cycle(price, exchange).await
                                    {
                                        log::error!("Error during capitulation cycle: {}", e);
                                    }

                                    // Run Main Ranger Strategy; Comment out for now.
                                    // if let Err(e) = self.run_cycle(price, exchange).await {
                                    //     log::error!("Error during trading cycle: {}", e);
                                    // }
                                }

                                // Periodic cumulative stats check (midnight)
                                if Utc::now().date_naive() != last_midnight_check.date_naive()
                                    && Helper::is_midnight()
                                {
                                    warn!("It's midnight now! Processing weekly/monthly stats...");
                                    if let Err(e) = Graph::prepare_cumulative_weekly_monthly(
                                        &mut graph,
                                        self.redis_conn.clone(),
                                    )
                                    .await
                                    {
                                        log::error!("Failed to process cumulative stats: {}", e);
                                    }
                                    last_midnight_check = Utc::now();
                                }
                            }
                            std::result::Result::Err(e) => {
                                log::error!("WebSocket ticker stream error: {}", e);
                                break; // Break the inner loop to trigger reconnection
                            }
                        }
                    }
                    warn!("WebSocket stream closed. Attempting to reconnect...");
                }
                std::result::Result::Err(e) => {
                    log::error!(
                        "Failed to subscribe to tickers: {}. Retrying in {}s...",
                        e,
                        backoff_secs
                    );
                }
            }

            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = std::cmp::min(backoff_secs * 2, max_backoff);
        }
    }
}
