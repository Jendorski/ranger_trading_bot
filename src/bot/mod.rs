use anyhow::{Ok, Result};
use chrono::{DateTime, Utc};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::exchange::{Exchange, OrderSide};
use redis::AsyncCommands;

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
                    low: 105_169.9,
                    high: 106_097.8,
                },
                Zone {
                    low: 107_169.9,
                    high: 108_608.8,
                },
                Zone {
                    low: 111_005.0,
                    high: 111_108.6,
                },
                Zone {
                    low: 111_715.9,
                    high: 112_064.8,
                },
                // Zone {
                //     low: 114_684.1,
                //     high: 115_097.6,
                // },
                // Zone {
                //     low: 116_764.4,
                //     high: 117_233.8,
                // },
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
                    low: 117_500.0,
                    high: 118_008.3,
                },
                // Zone {
                //     low: 116_885.0,
                //     high: 117_434.0,
                // },
                // Zone {
                //     low: 115_385.0,
                //     high: 115_505.2,
                // },
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
                    low: 108_608.0,
                    high: 108_900.0,
                },
                Zone {
                    low: 109_108.0,
                    high: 109_486.0,
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
    pub position: Position,
    pub entry_price: f64,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub entry_time: DateTime<Utc>,
    pub exit_price: f64,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub exit_time: DateTime<Utc>,
    pub pnl: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OpenPosition {
    pub id: Uuid,         // unique identifier
    pub pos: Position,    // Long / Short
    pub entry_price: f64, // price at which we entered
    pub position_size: f64,
    #[serde(with = "chrono::serde::ts_milliseconds")] // store as epoch ms
    pub entry_time: DateTime<Utc>, // UTC timestamp of entry
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
            position_size: 0.001,
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
        id: Uuid,
    ) -> Result<OpenPosition> {
        let key = format!("trading::{}", id);

        let open_pos: String = conn.get(key).await?;

        Ok(serde_json::from_str(&open_pos)?)
    }

    async fn store_open_position(
        mut conn: redis::aio::MultiplexedConnection,
        open_pos: OpenPosition,
    ) -> Result<()> {
        let key = format!("trading::{}", open_pos.id);

        let _: () = conn.set(key, open_pos.as_str()).await?;

        Ok(())
    }
}

/// Trading state – we keep track of whether we have an open position
#[derive(Debug)]
pub struct Bot {
    pub current_pos_id: Uuid,

    pub open_pos: OpenPosition,

    // pub sl: f64,
    /// None if no position; Some(OrderSide::Buy) means long, Sell → short
    pub pos: Position, //Option<OrderSide>,

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

        let open_pos = OpenPosition::load_open_position(&mut conn, current_pos_id)
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
        let json: String = conn.get("trading_bot:zones").await?;
        Ok(serde_json::from_str(&json)?)
    }

    pub async fn load_position(conn: &mut redis::aio::MultiplexedConnection) -> Result<Position> {
        let opt: Option<String> = conn.get("trading_bot:position").await?;

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
            .set("trading_bot:position", pos.as_str())
            .await?;

        OpenPosition::store_open_position(self.redis_conn.clone(), open_pos).await?;

        Ok(())
    }

    /// Store *one* closed position in the list named `"closed_positions"`.
    pub async fn store_closed_position(
        conn: &mut redis::aio::MultiplexedConnection,
        pos: &ClosedPosition,
    ) -> Result<()> {
        let key = "closed_positions";
        let json = serde_json::to_string(pos)?;

        // LPUSH pushes to the **left** of the list – newest element first
        let _: () = conn.lpush(key, json).await?;

        // OPTIONAL: keep only the last N trades (e.g. 10 000)
        // conn.ltrim(key, 0, 9999).await?;

        Ok(())
    }

    /// Profit / loss for an open trade given the exit price.
    /// Positive → you made money, negative → you lost.
    pub fn compute_pnl(entry: &OpenPosition, exit_price: f64) -> f64 {
        match entry.pos {
            Position::Long => exit_price - entry.entry_price,
            Position::Short => entry.entry_price - exit_price,
            Position::Flat => 0.00,
        }
    }

    pub async fn run_cycle(
        &mut self,
        price: f64,
        size: f64,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        //info!("----Cycle start-----");
        info!("Price = {:.2} | State = {:?}", price, self.pos);

        match self.pos {
            Position::Flat => {
                if self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    info!("Entering LONG at {:.2}", price);
                    let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;
                    info!("Long executed at {:.2}", exec_price);
                    self.pos = Position::Long;
                    self.open_pos = OpenPosition {
                        id: Uuid::new_v4(),
                        pos: Position::Long,
                        entry_price: price,
                        position_size: size,
                        entry_time: Utc::now(),
                    };
                    self.current_pos_id = self.open_pos.id
                } else if self.zones.short_zones.iter().any(|z| z.contains(price)) {
                    info!("Entering SHORT at {:.2}", price);
                    let exec_price = exchange.place_market_order(OrderSide::Sell, size).await?;
                    info!("Short executed at {:.2}", exec_price);
                    self.pos = Position::Short;
                    self.open_pos = OpenPosition {
                        id: Uuid::new_v4(),
                        pos: Position::Short,
                        entry_price: price,
                        position_size: size,
                        entry_time: Utc::now(),
                    };
                    self.current_pos_id = self.open_pos.id;
                } else {
                    warn!("Price {:.2} out of any zone -- staying flat", price);
                }
            }

            Position::Long => {
                // 2️⃣ Take‑profit: exit long when we hit the short zone.
                if self.zones.short_zones.iter().any(|z| z.contains(price)) {
                    info!("Taking profit on LONG at {:.2}", price);
                    let exec_price = exchange.place_market_order(OrderSide::Sell, size).await?;
                    info!("Closed LONG at {:.2}", exec_price);
                    self.pos = Position::Flat;
                    let closed_pos = ClosedPosition {
                        id: self.open_pos.id,
                        entry_price: self.open_pos.entry_price,
                        exit_price: price,
                        exit_time: Utc::now(),
                        position: Position::Long,
                        entry_time: self.open_pos.entry_time,
                        pnl: Bot::compute_pnl(&self.open_pos, price),
                    };
                    let _ = Bot::store_closed_position(&mut self.redis_conn, &closed_pos).await;
                }
            }

            Position::Short => {
                // 3️⃣ Cover: exit short when we hit the long zone.
                if self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    info!("Covering SHORT at {:.2}", price);
                    let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;
                    info!("Covered SHORT at {:.2}", exec_price);
                    self.pos = Position::Flat;
                    let closed_pos = ClosedPosition {
                        id: self.open_pos.id,
                        entry_price: self.open_pos.entry_price,
                        exit_price: price,
                        exit_time: Utc::now(),
                        position: Position::Short,
                        entry_time: self.open_pos.entry_time,
                        pnl: Bot::compute_pnl(&self.open_pos, price),
                    };
                    let _ = Bot::store_closed_position(&mut self.redis_conn, &closed_pos).await;
                }
            }
        }
        self.store_position(self.pos, self.open_pos).await?;
        Ok(())
    }
}
