use anyhow::Result;
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use log::{info, warn};
use redis::{AsyncCommands, RedisError};
use serde::{Deserialize, Serialize};
use std::ops::Div;
use std::result::Result::Ok;
use std::sync::Arc;
use uuid::Uuid;

use crate::config::Config;
use crate::exchange::Exchange;
use crate::exchange::HttpExchange;
use crate::exchange::bitget::PlaceOrderData;
use crate::helper::TRADING_BOT_LOSS_COUNT;
use crate::helper::TRADING_PARTIAL_PROFIT_TARGET;
use crate::helper::{
    Helper, PartialProfitTarget, TRADING_BOT_ACTIVE, TRADING_BOT_CLOSE_POSITIONS,
    TRADING_BOT_POSITION, TRADING_BOT_ZONES, TRADING_CAPITAL,
};
use crate::trackers::smart_money_concepts::SmcEngine;

//pub mod scalper;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Zone {
    pub low: f64,
    pub high: f64,
}

impl Zone {
    /// Returns true if price lies in the zone
    #[inline]
    pub fn contains(&self, price: f64) -> bool {
        price >= self.low && price <= self.high
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zones {
    pub long_zones: Vec<Zone>,
    pub short_zones: Vec<Zone>,
}
/**
 * For Zones, add a 1000 difference between a long and short zone.
 */
impl Default for Zones {
    fn default() -> Self {
        Self {
            long_zones: vec![
                Zone {
                    low: 74_306.80,
                    high: 74_394.80,
                },
                Zone {
                    low: 79_981.80,
                    high: 80_102.80,
                },
                Zone {
                    low: 85_301.80,
                    high: 85_402.80,
                },
                // Zone {
                //     low: 89_301.80,
                //     high: 89_402.80,
                // },
                // Zone {
                //     low: 91_106.80,
                //     high: 91_134.80,
                // },
                // Zone {
                //     low: 93_030.10,
                //     high: 93_179.60,
                // },
                // Zone {
                //     low: 99_079.40,
                //     high: 99_299.00,
                // },
                // //These zones are chop city
                // // Zone {
                // //     low: 102_979.9,
                // //     high: 103_057.8,
                // // },
                // // Zone {
                // //     low: 106_496.8,
                // //     high: 106_596.8,
                // // },
                // Zone {
                //     low: 105_118.9,
                //     high: 105_240.10,
                // },
                Zone {
                    low: 109_018.9,
                    high: 109_122.8,
                },
                Zone {
                    low: 113_293.9,
                    high: 113_393.8,
                },
                Zone {
                    low: 114_548.9,
                    high: 114_677.8,
                },
                Zone {
                    low: 116_344.4,
                    high: 116_464.4,
                },
                Zone {
                    low: 121_100.0,
                    high: 121_350.0,
                },
                Zone {
                    low: 122_400.0,
                    high: 122_350.0,
                },
                Zone {
                    low: 123_100.0,
                    high: 123_150.0,
                },
                Zone {
                    low: 124_600.0,
                    high: 124_650.0,
                },
                Zone {
                    low: 124_199.0,
                    high: 125_220.0,
                },
            ],
            short_zones: vec![
                Zone {
                    low: 125_797.0,
                    high: 125_897.0,
                },
                Zone {
                    low: 125_097.0,
                    high: 125_197.0,
                },
                Zone {
                    low: 124_500.0,
                    high: 124_540.0,
                },
                Zone {
                    low: 123_990.0,
                    high: 124_032.0,
                },
                Zone {
                    low: 122_900.0,
                    high: 123_000.0,
                },
                Zone {
                    low: 120_931.4,
                    high: 120_170.1,
                },
                Zone {
                    low: 119_409.0,
                    high: 119_479.7,
                },
                Zone {
                    low: 117_514.0,
                    high: 118_008.3,
                },
                Zone {
                    low: 115_585.0,
                    high: 116_085.2,
                },
                Zone {
                    low: 114_316.0,
                    high: 114_486.2,
                },
                Zone {
                    low: 112_990.0,
                    high: 113_100.0,
                },
                Zone {
                    low: 108_511.0,
                    high: 108_646.0,
                },
                Zone {
                    low: 104_511.00,
                    high: 104_596.30,
                },
                //These zones are chop city
                // Zone {
                //     low: 106_384.0,
                //     high: 106_484.0,
                // },
                // Zone {
                //     low: 102_801.0,
                //     high: 102_850.0,
                // },
                Zone {
                    low: 98_030.10,
                    high: 98_079.60,
                },
                Zone {
                    low: 93_630.10,
                    high: 93_679.60,
                },
                Zone {
                    low: 92_630.10,
                    high: 92_679.60,
                },
                Zone {
                    low: 89_906.80,
                    high: 90_008.60,
                },
                // Zone {
                //     low: 84_906.80,
                //     high: 85_098.60,
                // },
                Zone {
                    low: 79_806.80,
                    high: 80_098.60,
                },
                Zone {
                    low: 73_906.80,
                    high: 73_979.60,
                },
            ],
        }
    }
}

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
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ClosedPosition {
    pub id: uuid::Uuid,
    pub position: Option<Position>,
    pub side: Option<Position>,
    pub entry_price: f64,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub entry_time: DateTime<Utc>,
    pub exit_price: f64,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub exit_time: DateTime<Utc>,
    pub pnl: f64,
    pub quantity: Option<f64>,
    //pub tp: Option<f64>,
    pub sl: Option<f64>,
    pub roi: Option<f64>,
    pub leverage: Option<f64>,
    pub margin: Option<f64>,
}

impl ClosedPosition {
    pub fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OpenPosition {
    pub id: Uuid,         // unique identifier
    pub pos: Position,    // Long / Short
    pub entry_price: f64, // price at which we entered
    pub position_size: f64,
    #[serde(with = "chrono::serde::ts_milliseconds")] // store as epoch ms
    pub entry_time: DateTime<Utc>, // UTC timestamp of entry
    pub tp: Option<f64>,
    pub sl: Option<f64>,
    pub margin: Option<f64>,
    pub quantity: Option<f64>,
    pub leverage: Option<f64>,
    pub risk_pct: Option<f64>,
}

impl OpenPosition {
    pub fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap()
    }

    fn default_open_position() -> OpenPosition {
        OpenPosition {
            id: Uuid::nil(),
            pos: Position::Flat,
            entry_price: 0.00,
            entry_time: Utc::now(),
            position_size: 0.015,
            tp: Some(0.00),
            sl: Some(0.00),
            margin: Some(50.00),
            quantity: Some(0.015),
            risk_pct: Some(0.05),
            leverage: Some(35.00),
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

    // a *mutable* reference to the redis connection
    redis_conn: redis::aio::MultiplexedConnection,

    config: &'a Config,

    current_margin: f64,

    partial_profit_target: Vec<PartialProfitTarget>,
    //pub zone: Zone,
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

        // let default_zone = Zone {
        //     high: 0.00,
        //     low: 0.00,
        // };

        Ok(Self {
            pos,
            zones,
            redis_conn: conn,
            open_pos,
            config,
            current_margin,
            partial_profit_target,
            //default_zone,
            //zone: default_zone,
            loss_count,
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

    pub fn prepare_open_position(
        &mut self,
        pos: Position,
        entry_price: f64,
        leverage: f64,
        risk_pct: f64,
    ) -> OpenPosition {
        let current_margin = self.current_margin;
        let sl = Helper::stop_loss_price(entry_price, current_margin, leverage, risk_pct, pos);
        let qty = Helper::contract_amount(entry_price, current_margin, leverage);
        let tp = self
            .partial_profit_target
            .last()
            .unwrap_or(&PartialProfitTarget {
                target_price: 1.11,
                fraction: 0.0,
                sl: 1.11,
            })
            .target_price;
        OpenPosition {
            id: Uuid::new_v4(),
            pos: pos,
            entry_price: entry_price,
            position_size: qty, //does the same thing as quantity :(
            entry_time: Utc::now(),
            tp: Some(tp),
            sl: Some(sl),
            margin: Some(current_margin),
            quantity: Some(qty),
            leverage: Some(leverage),
            risk_pct: Some(risk_pct),
        }
    }

    async fn delete_partial_profit_target(&mut self) -> Result<()> {
        let _: () = self.redis_conn.del(TRADING_PARTIAL_PROFIT_TARGET).await?;

        self.partial_profit_target = [].to_vec();

        Ok(())
    }

    pub async fn close_long_position(&mut self, price: f64) {
        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos.margin.unwrap_or(self.config.margin),
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
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;

        //update the margin based on the pnl
        let _ = Self::prepare_current_margin(self, pnl).await;

        let _ = self.store_loss_count(pnl).await;

        //Track loss count
        let total_profit_count = 4;
        if self.partial_profit_target.len() == total_profit_count {
            //This means that we did not hit any of the targets
            self.loss_count += 1;
            info!("Loss count: {}", self.loss_count);
            let total_loss_count = 2;
            if self.loss_count >= total_loss_count {
                self.pos = Position::Flat;
                let _ = self.store_loss_count(pnl).await;
            }
        }
    }

    async fn store_loss_count(&mut self, pnl: f64) -> Result<()> {
        if pnl.is_sign_negative() || pnl < 0.00 {
            self.loss_count += 1;
            if self.loss_count >= 2 {
                //Store the loss count in redis for 12hours
                if let Err(e) = self
                    .redis_conn
                    .set_ex::<_, _, ()>(TRADING_BOT_LOSS_COUNT, self.loss_count, 14400) //4hours reset
                    .await
                {
                    warn!("Failed to store loss count: {}", e);
                }
            }
        }
        Ok(())
    }

    pub async fn load_current_margin(
        redis_conn: &mut redis::aio::MultiplexedConnection,
        config: &'a Config,
    ) -> f64 {
        let key = TRADING_CAPITAL;

        let raw_margin: Result<Option<String>, RedisError> = redis_conn.get(key).await;

        let mut margin = match raw_margin {
            Ok(Some(raw_margin)) => {
                serde_json::from_str::<f64>(&raw_margin).unwrap_or_else(|_| config.margin)
            }
            Ok(None) => config.margin,
            Err(_) => config.margin,
        };

        if margin <= 5.00 {
            warn!("margin as we know it, is rekt, {:2}", margin);
            margin = config.margin;
            return margin;
        }

        return margin;
    }

    pub async fn prepare_current_margin(&mut self, pnl: f64) -> f64 {
        let mut current_margin = Self::load_current_margin(&mut self.redis_conn, self.config).await;

        current_margin += pnl;
        info!("current_margin, {:2}", current_margin);

        if current_margin <= 5.00 {
            warn!("current_margin is rekt, {:2}", current_margin);
            current_margin = self.config.margin;
            self.open_pos.margin = Some(current_margin);
        }

        self.current_margin = current_margin;

        let _ = Self::store_current_margin(current_margin, &mut self.redis_conn).await;
        let _ = OpenPosition::store_open_position(self.redis_conn.clone(), self.open_pos).await;

        return current_margin;
    }

    async fn store_current_margin(
        current_margin: f64,
        conn: &mut redis::aio::MultiplexedConnection,
    ) -> Result<()> {
        let json = serde_json::to_string(&current_margin).expect("Failed to serialize margin");

        let _: () = conn.set(TRADING_CAPITAL, json).await?;

        Ok(())
    }

    pub async fn close_short_position(&mut self, price: f64) {
        let pnl = Helper::compute_pnl(
            self.open_pos.pos,
            self.open_pos.entry_price,
            self.open_pos.position_size,
            price,
        );
        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos.margin.unwrap_or(self.config.margin),
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
            side: None,
            entry_time: self.open_pos.entry_time,
            pnl,
            quantity: Some(self.open_pos.position_size),
            sl: self.open_pos.sl,
            roi: Some(roi),
            leverage: self.open_pos.leverage,
            margin: self.open_pos.margin,
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;

        //update the margin based on the pnl
        let _ = Self::prepare_current_margin(self, pnl).await;
    }

    pub async fn take_profit_on_long(&mut self, price: f64, exchange: &dyn Exchange) -> Result<()> {
        info!("Ranger Taking profit on LONG at {:.2}", price);

        let exec_price: PlaceOrderData = exchange.modify_market_order(self.open_pos).await?;

        info!("Ranger Closed LONG at {:?}", exec_price);

        Self::close_long_position(self, price).await;

        self.pos = Position::Flat;

        Ok(())
    }

    async fn take_partial_profit_on_long(
        &mut self,
        price: f64,
        target: PartialProfitTarget,
    ) -> Result<()> {
        let mut remaining_size = self.open_pos.quantity.unwrap_or_default();
        let qty_to_close = target.fraction * remaining_size;

        if qty_to_close <= 0.0000 {
            Self::close_long_position(self, price).await;
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

        if remaining_size <= 0.0000 {
            self.open_pos.quantity = Some(remaining_size);
            self.open_pos.position_size = remaining_size;
            Self::close_long_position(self, price).await;
        }

        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos.margin.unwrap_or(self.config.margin),
            self.open_pos.entry_price,
            self.pos,
            qty_to_close,
            price,
        );
        let pnl = Helper::compute_pnl(self.pos, self.open_pos.entry_price, qty_to_close, price);
        let closed_pos = ClosedPosition {
            id: self.open_pos.id,
            entry_price: self.open_pos.entry_price,
            exit_price: price,
            exit_time: Utc::now(),
            position: Some(Position::Long),
            side: None,
            entry_time: self.open_pos.entry_time,
            pnl,
            quantity: Some(qty_to_close),
            sl: self.open_pos.sl,
            roi: Some(roi),
            leverage: self.open_pos.leverage,
            margin: self.open_pos.margin,
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;

        //update the margin based on the pnl
        let _ = Self::prepare_current_margin(self, pnl).await;

        self.open_pos = OpenPosition {
            id: self.open_pos.id,
            pos: self.open_pos.pos,
            entry_price: self.open_pos.entry_price,
            position_size: remaining_size,
            entry_time: self.open_pos.entry_time,
            tp: Some(target.target_price),
            sl: Some(target.sl),
            margin: self.open_pos.margin,
            quantity: Some(remaining_size),
            leverage: self.open_pos.leverage,
            risk_pct: self.open_pos.risk_pct,
        };

        warn!("NEW SL for LONG is: {:?}", target.sl);
        self.store_position(self.pos, self.open_pos).await?;
        Ok(())
    }

    async fn take_partial_profit_on_short(
        &mut self,
        price: f64,
        target: PartialProfitTarget,
    ) -> Result<()> {
        let mut remaining_size = self.open_pos.quantity.unwrap_or_default();
        let qty_to_close = target.fraction * remaining_size;

        if qty_to_close <= 0.0000 {
            Self::close_short_position(self, price).await;
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

        if remaining_size <= 0.0000 {
            self.open_pos.quantity = Some(remaining_size);
            self.open_pos.position_size = remaining_size;
            Self::close_short_position(self, price).await;
        }

        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos.margin.unwrap_or(self.config.margin),
            self.open_pos.entry_price,
            self.pos,
            qty_to_close,
            price,
        );
        let pnl = Helper::compute_pnl(self.pos, self.open_pos.entry_price, qty_to_close, price);
        let closed_pos = ClosedPosition {
            id: self.open_pos.id,
            entry_price: self.open_pos.entry_price,
            exit_price: price,
            exit_time: Utc::now(),
            position: Some(Position::Short),
            side: None,
            entry_time: self.open_pos.entry_time,
            pnl,
            quantity: Some(qty_to_close),
            sl: self.open_pos.sl,
            roi: Some(roi),
            leverage: self.open_pos.leverage,
            margin: self.open_pos.margin,
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;

        //update the margin based on the pnl
        let _ = Self::prepare_current_margin(self, pnl).await;

        self.open_pos = OpenPosition {
            id: self.open_pos.id,
            pos: self.open_pos.pos,
            entry_price: self.open_pos.entry_price,
            position_size: remaining_size,
            entry_time: self.open_pos.entry_time,
            tp: Some(target.target_price),
            sl: Some(target.sl),
            margin: self.open_pos.margin,
            quantity: Some(remaining_size),
            leverage: self.open_pos.leverage,
            risk_pct: self.open_pos.risk_pct,
        };
        self.store_position(self.pos, self.open_pos).await?;

        warn!("NEW SL for SHORT is: {:?}", target.sl);

        Ok(())
    }

    pub async fn take_profit_on_short(
        &mut self,
        price: f64,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        info!("Ranger Covering SHORT at {:.2}", price);

        let exec_price: PlaceOrderData = exchange.modify_market_order(self.open_pos).await?;

        info!("Ranger Covered SHORT at {:?}", exec_price);

        Self::close_short_position(self, price).await;

        self.pos = Position::Flat;

        Ok(())
    }

    fn determine_profit_difference(&mut self, entry_price: f64, pos: Position) -> f64 {
        let mut the_zone = Zone {
            high: 0.00,
            low: 0.00,
        };

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
            the_zone = *valid_zones
                .into_iter()
                .min_by(|a, b| {
                    let dist_a = (a.low - entry_price).abs();
                    let dist_b = (b.low - entry_price).abs();
                    dist_a.partial_cmp(&dist_b).unwrap()
                })
                .unwrap_or(&Zone {
                    low: 0.00,
                    high: 0.00,
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
            the_zone = *valid_zones
                .into_iter()
                .min_by(|a, b| {
                    let dist_a = (entry_price - a.high).abs();
                    let dist_b = (entry_price - b.high).abs();
                    dist_a.partial_cmp(&dist_b).unwrap()
                })
                .unwrap_or(&Zone {
                    low: 0.00,
                    high: 0.00,
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

        let ppt = Helper::build_profit_targets(entry_price, ranger_price_difference, pos);

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

    async fn evaluate_long_partial_profit(&mut self, price: f64) -> Result<()> {
        if self.partial_profit_target.len() == 0 {
            info!(
                "ALL TARGETS HIT FOR LONG!: {:?}",
                self.partial_profit_target
            );
            self.pos = Position::Flat;
        }

        let idx_opt = self
            .partial_profit_target
            .iter()
            .position(|t| price >= t.target_price);

        let idx = idx_opt.unwrap_or(usize::MAX);

        if idx == usize::MAX {
            return Ok(());
        }

        let target = self.partial_profit_target[idx].clone();

        if target.target_price == 0.00
            || !target.target_price.is_finite()
            || target.target_price == 1.11
        {
            return Ok(());
        }

        info!(
            "LONG: Taking Partial Profits here.... {:?}, Take profit targets: {:?}",
            price, self.partial_profit_target
        );
        let _: () = Self::take_partial_profit_on_long(self, price, target).await?;

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

    async fn evaluate_short_partial_profit(&mut self, price: f64) -> Result<()> {
        if self.partial_profit_target.len() == 0 {
            info!(
                "ALL TARGETS HIT FOR SHORT!: {:?}",
                self.partial_profit_target
            );
            self.pos = Position::Flat;
        }

        let idx_opt = self
            .partial_profit_target
            .iter()
            .position(|t| price <= t.target_price);

        let idx = idx_opt.unwrap_or(usize::MAX);

        if idx == usize::MAX {
            return Ok(());
        }

        let target = self.partial_profit_target[idx].clone();

        if target.target_price == 0.00
            || !target.target_price.is_finite()
            || target.target_price == 1.11
        {
            return Ok(());
        }

        info!(
            "SHORT: Taking Partial Profits here.... {:?}, Take profit targets: {:?}",
            price, self.partial_profit_target
        );
        let _: () = Self::take_partial_profit_on_short(self, price, target).await?;

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

    pub async fn test(&mut self) -> Result<()> {
        self.open_pos = OpenPosition {
            id: Uuid::new_v4(),
            pos: Position::Long,
            entry_price: 86800.11,
            position_size: 0.09,
            entry_time: Utc::now(),
            tp: Some(90000.4),
            sl: Some(83000.4),
            margin: Some(50.01),
            quantity: Some(0.09),
            leverage: Some(20.01),
            risk_pct: Some(0.075),
        };
        //Get the price from the exchange API
        let exchange = Arc::new(HttpExchange {
            client: reqwest::Client::new(),
            symbol: Config::from_env().unwrap().symbol,
        });
        let price = exchange.place_market_order(self.open_pos).await.unwrap();
        info!("price: {:?}", price);
        //let _: () = SmcEngine::smc_find_targets(&mut self.redis_conn, 82000.00).await;
        Ok(())
    }

    pub async fn run_cycle(&mut self, price: f64, exchange: &dyn Exchange) -> Result<()> {
        if price == 1.11 {
            warn!("Price failure! -> {:?}", price);
            return Ok(());
        }

        self.loss_count = Self::load_loss_count(&mut self.redis_conn).await?;
        if self.loss_count >= 2 {
            warn!("Loss count reached 2, skipping cycle");
            return Ok(());
        }

        //Load the zones, because it's usually updated, periodically.
        self.zones = Bot::load_zones(&mut self.redis_conn)
            .await
            .unwrap_or(Zones::default());

        warn!("Ranger State = {:?}", self.pos);

        match self.pos {
            Position::Flat => {
                if price != 1.11 && self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    let exec_price: PlaceOrderData =
                        exchange.place_market_order(self.open_pos).await?;
                    if exec_price.client_oid == "Failed to place order" {
                        warn!("Failed to place order");
                        //return Ok(());
                    }

                    info!("Ranger Entering LONG at {:.2}", price);
                    let _: () = Self::delete_partial_profit_target(self).await?;

                    info!("Ranger Long executed at {:?}", exec_price);

                    self.pos = Position::Long;

                    let _: Result<()> =
                        Self::store_partial_profit_targets(self, price, self.pos).await;

                    self.open_pos = Self::prepare_open_position(
                        self,
                        self.pos,
                        price,
                        self.config.leverage,
                        self.config.ranger_risk_pct,
                    );
                } else if price != 1.11 && self.zones.short_zones.iter().any(|z| z.contains(price))
                {
                    info!("Ranger Entering SHORT at {:.2}", price);
                    let _: () = Self::delete_partial_profit_target(self).await?;

                    let exec_price: PlaceOrderData =
                        exchange.place_market_order(self.open_pos).await?;

                    if exec_price.client_oid == "Failed to place order" {
                        warn!("Failed to place order");
                        //return Ok(());
                    }

                    info!("Ranger Short executed at {:?}", exec_price);

                    self.pos = Position::Short;

                    self.open_pos = Self::prepare_open_position(
                        self,
                        Position::Short,
                        price,
                        self.config.leverage,
                        self.config.ranger_risk_pct,
                    );

                    let _: Result<()> =
                        Self::store_partial_profit_targets(self, price, self.pos).await;
                } else {
                    //Track for new zone targets
                    warn!("Price {:.2} out of any Ranger zone -- staying flat", price);
                    //let _: () = SmcEngine::smc_find_targets(&mut self.redis_conn, price).await;
                }
            }

            Position::Long => {
                //Trigger SL if it's met
                let in_sl = Helper::stop_loss_price(
                    self.open_pos.entry_price,
                    self.config.margin,
                    self.config.leverage,
                    self.config.risk_pct,
                    Position::Long,
                );
                let ssl_hit = Helper::ssl_hit(price, self.pos, self.open_pos.sl.unwrap_or(in_sl));

                if ssl_hit {
                    Self::close_long_position(self, price).await;

                    warn!(
                        "SL for Ranger Long Position entered at {:2}, with SL triggered at {:2}",
                        self.open_pos.entry_price, price
                    );

                    self.pos = Position::Flat;
                }

                // 2️⃣ Take‑profit: exit long when we hit the short zone.
                if self.zones.short_zones.iter().any(|z| z.contains(price)) {
                    Self::take_profit_on_long(self, price, exchange).await?;
                }

                let _ = Self::evaluate_long_partial_profit(self, price).await?;
            }

            Position::Short => {
                //Trigger SL if it's met
                let in_sl = Helper::stop_loss_price(
                    self.open_pos.entry_price,
                    self.config.margin,
                    self.config.leverage,
                    self.config.risk_pct,
                    Position::Short,
                );
                let ssl_hit = Helper::ssl_hit(price, self.pos, self.open_pos.sl.unwrap_or(in_sl));

                if ssl_hit {
                    Self::close_short_position(self, price).await;

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

                let _ = Self::evaluate_short_partial_profit(self, price).await;
            }
        }
        self.store_position(self.pos, self.open_pos).await?;
        Ok(())
    }
}
