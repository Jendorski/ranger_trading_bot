use anyhow::{Ok, Result};
use chrono::{DateTime, Utc};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::exchange::{Exchange, OrderSide};
use crate::helper::{
    Helper, TRADING_BOT_ACTIVE, TRADING_BOT_CLOSE_POSITIONS, TRADING_BOT_POSITION,
    TRADING_BOT_ZONES,
};
use redis::AsyncCommands;

pub mod scalper;

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

impl Default for Zones {
    fn default() -> Self {
        Self {
            long_zones: vec![
                Zone {
                    low: 98_000.0,
                    high: 100_000.0,
                },
                Zone {
                    low: 102_169.9,
                    high: 102_297.8,
                },
                Zone {
                    low: 102_979.9,
                    high: 103_057.8,
                },
                // Zone {
                //     low: 105_969.9,
                //     high: 106_097.8,
                // },
                Zone {
                    low: 108_108.8,
                    high: 108_308.8,
                },
                Zone {
                    low: 109_499.9,
                    high: 109_518.8,
                },
                Zone {
                    low: 111_005.0,
                    high: 111_108.6,
                },
                Zone {
                    low: 111_710.9,
                    high: 112_064.8,
                },
                Zone {
                    low: 114_684.1,
                    high: 115_097.6,
                },
                Zone {
                    low: 116_764.4,
                    high: 117_233.8,
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
                // Zone {
                //     low: 117_500.0,
                //     high: 118_008.3,
                // },
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
                    low: 111_724.0,
                    high: 111_615.9,
                },
                Zone {
                    low: 110_500.0,
                    high: 110_790.9,
                },
                Zone {
                    low: 109_108.0,
                    high: 109_386.0,
                },
                Zone {
                    low: 108_608.0,
                    high: 108_900.0,
                },
                Zone {
                    low: 107_201.0,
                    high: 107_739.0,
                },
                // Zone {
                //     low: 105_798.0,
                //     high: 105_880.0,
                // },
                Zone {
                    low: 101_908.0,
                    high: 102_000.0,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OpenPosition {
    pub id: Uuid,         // unique identifier
    pub pos: Position,    // Long / Short
    pub entry_price: f64, // price at which we entered
    pub position_size: f64,
    #[serde(with = "chrono::serde::ts_milliseconds")] // store as epoch ms
    pub entry_time: DateTime<Utc>, // UTC timestamp of entry
    pub sl: Option<f64>,
    pub margin: Option<f64>,
    pub quantity: Option<f64>,
    pub leverage: Option<f64>,
    pub risk_pct: Option<f64>,
}

impl OpenPosition {
    fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap()
    }

    fn default_open_position() -> OpenPosition {
        OpenPosition {
            id: Uuid::nil(),
            pos: Position::Flat,
            entry_price: 0.00,
            entry_time: Utc::now(),
            position_size: 0.015,
            sl: Some(0.00),
            margin: Some(50.00),
            quantity: Some(0.015),
            risk_pct: Some(0.05),
            leverage: Some(35.00),
        }
    }

    async fn load_current_position_id(
        conn: &mut redis::aio::MultiplexedConnection,
    ) -> Result<Uuid> {
        let json: String = conn.get("current_position_id").await?;
        Ok(serde_json::from_str(&json)?)
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
pub struct Bot {
    pub current_pos_id: Uuid,

    pub open_pos: OpenPosition,

    pub pos: Position,

    pub zones: Zones,

    // a *mutable* reference to the redis connection
    redis_conn: redis::aio::MultiplexedConnection,
}

impl Bot {
    pub async fn new(mut conn: redis::aio::MultiplexedConnection) -> Result<Self> {
        let pos: Position = Self::load_position(&mut conn)
            .await
            .unwrap_or_else(|_| Position::Flat);

        let zones: Zones = Self::load_zones(&mut conn)
            .await
            .unwrap_or_else(|_| Zones::default());

        let current_pos_id = OpenPosition::load_current_position_id(&mut conn)
            .await
            .unwrap_or_else(|_| Uuid::nil());

        let open_pos = OpenPosition::load_open_position(&mut conn)
            .await
            .unwrap_or_else(|_| OpenPosition::default_open_position());

        Ok(Self {
            pos,
            zones,
            redis_conn: conn,
            current_pos_id,
            open_pos,
        })
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
        position_size: f64,
        margin: f64,
        leverage: f64,
        risk_pct: f64,
    ) -> OpenPosition {
        let sl = Helper::stop_loss_price(entry_price, margin, leverage, risk_pct, pos);
        let qty = Helper::contract_amount(entry_price, margin, leverage);
        OpenPosition {
            id: Uuid::new_v4(),
            pos: pos,
            entry_price: entry_price,
            position_size, //does the same thing as quantity :(
            entry_time: Utc::now(),
            sl: Some(sl),
            margin: Some(margin),
            quantity: Some(qty),
            leverage: Some(leverage),
            risk_pct: Some(risk_pct),
        }
    }

    pub async fn close_long_position(&mut self, price: f64, config: &mut Config) {
        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos.margin.unwrap_or(config.margin),
            self.open_pos.entry_price,
            self.pos,
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
            pnl: Helper::compute_pnl(
                self.pos,
                self.open_pos.entry_price,
                self.open_pos.position_size,
                price,
            ),
            quantity: Some(self.open_pos.position_size),
            sl: self.open_pos.sl,
            roi: Some(roi),
            leverage: self.open_pos.leverage,
            margin: self.open_pos.margin,
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;
    }

    pub async fn close_short_position(&mut self, price: f64, config: &mut Config) {
        let pnl = Helper::compute_pnl(
            self.open_pos.pos,
            self.open_pos.entry_price,
            self.open_pos.position_size,
            price,
        );
        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.open_pos.margin.unwrap_or(config.margin),
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
    }

    pub async fn take_profit_on_long(
        &mut self,
        price: f64,
        size: f64,
        config: &mut Config,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        info!("Ranger Taking profit on LONG at {:.2}", price);

        let exec_price = exchange.place_market_order(OrderSide::Sell, size).await?;

        info!("Ranger Closed LONG at {:.2}", exec_price);

        Self::close_long_position(self, price, config).await;

        self.pos = Position::Flat;

        Ok(())
    }

    pub async fn take_profit_on_short(
        &mut self,
        price: f64,
        size: f64,
        config: &mut Config,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        info!("Ranger Covering SHORT at {:.2}", price);

        let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;

        info!("Ranger Covered SHORT at {:.2}", exec_price);

        Self::close_short_position(self, price, config).await;

        self.pos = Position::Flat;

        Ok(())
    }

    pub async fn run_cycle(
        &mut self,
        price: f64,
        exchange: &dyn Exchange,
        config: &mut Config,
    ) -> Result<()> {
        // info!("Price = {:.2} | State = {:?}", price, self.pos);
        warn!("Ranger State = {:?}", self.pos);

        let size = Helper::contract_amount(price, config.margin, config.leverage);

        match self.pos {
            Position::Flat => {
                if self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    info!("Ranger Entering LONG at {:.2}", price);

                    let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;
                    info!("Ranger Long executed at {:.2}", exec_price);

                    self.pos = Position::Long;

                    self.open_pos = Self::prepare_open_position(
                        self,
                        self.pos,
                        price,
                        size,
                        config.margin,
                        config.leverage,
                        config.ranger_risk_pct,
                    );

                    self.current_pos_id = self.open_pos.id
                } else if self.zones.short_zones.iter().any(|z| z.contains(price)) {
                    info!("Ranger Entering SHORT at {:.2}", price);

                    let exec_price = exchange.place_market_order(OrderSide::Sell, size).await?;

                    info!("Ranger Short executed at {:.2}", exec_price);

                    self.pos = Position::Short;

                    self.open_pos = Self::prepare_open_position(
                        self,
                        Position::Short,
                        price,
                        size,
                        config.margin,
                        config.leverage,
                        config.ranger_risk_pct,
                    );

                    self.current_pos_id = self.open_pos.id;
                } else {
                    //warn!("Price {:.2} out of any Ranger zone -- staying flat", price);
                }
            }

            Position::Long => {
                //Trigger SL if it's met
                let in_sl = Helper::stop_loss_price(
                    self.open_pos.entry_price,
                    config.margin,
                    config.leverage,
                    config.risk_pct,
                    Position::Long,
                );
                let ssl_hit = Helper::ssl_hit(price, self.pos, self.open_pos.sl.unwrap_or(in_sl));

                if ssl_hit {
                    Self::close_long_position(self, price, config).await;

                    warn!(
                        "SL for Ranger Long Position entered at {:2}, with SL triggered at {:2}",
                        self.open_pos.entry_price, price
                    );

                    self.pos = Position::Flat;
                }

                // 2️⃣ Take‑profit: exit long when we hit the short zone.
                if self.zones.short_zones.iter().any(|z| z.contains(price)) {
                    Self::take_profit_on_long(self, price, size, config, exchange).await?;
                }

                if config.ranger_price_difference.is_finite()
                    && config.ranger_price_difference > 0.00
                {
                    let config_diff = config.ranger_price_difference;
                    let min_config_diff = config_diff - 100.00;
                    let diff = Helper::calc_price_difference(
                        self.open_pos.entry_price,
                        price,
                        Position::Long,
                    );
                    info!(
                        "RANGER diff >= config_diff {:2.2} >= {:2.2}",
                        diff, config_diff
                    );

                    if diff >= config_diff || diff >= min_config_diff {
                        //Take your profits and get out!
                        Self::take_profit_on_long(
                            self,
                            price,
                            self.open_pos.position_size,
                            config,
                            exchange,
                        )
                        .await?;
                    }
                }
            }

            Position::Short => {
                //Trigger SL if it's met
                let in_sl = Helper::stop_loss_price(
                    self.open_pos.entry_price,
                    config.margin,
                    config.leverage,
                    config.risk_pct,
                    Position::Long,
                );
                let ssl_hit = Helper::ssl_hit(price, self.pos, self.open_pos.sl.unwrap_or(in_sl));

                if ssl_hit {
                    Self::close_short_position(self, price, config).await;

                    warn!(
                        "SL for Ranger Short Position entered at {:2}, with SL triggered at {:2}",
                        self.open_pos.entry_price, price
                    );

                    self.pos = Position::Flat;
                }

                // 3️⃣ Cover: exit short when we hit the long zone.
                if self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    Self::take_profit_on_short(self, price, size, config, exchange).await?;
                }

                if config.ranger_price_difference.is_finite()
                    && config.ranger_price_difference > 0.00
                {
                    let config_diff = config.ranger_price_difference;
                    let min_config_diff = config_diff - 100.00;
                    let diff = Helper::calc_price_difference(
                        self.open_pos.entry_price,
                        price,
                        Position::Short,
                    );
                    info!(
                        "RANGER diff >= config_diff {:2.2} >= {:2.2}",
                        diff, config_diff
                    );

                    if diff >= config_diff || diff >= min_config_diff {
                        //Take your profits and get out!
                        Self::take_profit_on_short(
                            self,
                            price,
                            self.open_pos.position_size,
                            config,
                            exchange,
                        )
                        .await?;
                    }
                }
            }
        }
        self.store_position(self.pos, self.open_pos).await?;
        Ok(())
    }
}
