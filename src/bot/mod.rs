use anyhow::{Ok, Result};
use log::{info, warn};
use serde::{Deserialize, Serialize};

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
                Zone {
                    low: 114_684.1,
                    high: 115_097.6,
                },
            ],
            short_zones: vec![
                Zone {
                    low: 122_900.0,
                    high: 123_000.0,
                },
                Zone {
                    low: 118_899.0,
                    high: 119_002.7,
                },
                Zone {
                    low: 117_814.0,
                    high: 118_008.3,
                },
                Zone {
                    low: 116_885.0,
                    high: 117_434.0,
                },
                Zone {
                    low: 115_385.0,
                    high: 115_505.2,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    // fn from_str(s: &str) -> Option<Self> {
    //     match s {
    //         "Flat" => Some(Position::Flat),
    //         "Long" => Some(Position::Long),
    //         "Short" => Some(Position::Short),
    //         _ => None,
    //     }
    // }
}

/// Trading state – we keep track of whether we have an open position
#[derive(Debug)]
pub struct Bot {
    //pub entry_pos: f64,

    // pub tp: f64,

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

        // let entry_pos: f64 = Self::load_entry_pos(&mut conn)
        //     .await
        //     .unwrap_or_else(|_| 0.00);

        Ok(Self {
            pos,
            zones,
            redis_conn: conn,
            //entry_pos,
        })
    }

    // async fn load_entry_pos(conn: &mut redis::aio::MultiplexedConnection) -> Result<f64> {
    //     let json: String = conn.get("trading_bot:entry_position").await?;
    //     Ok(serde_json::from_str(&json)?)
    // }

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

    async fn store_position(&mut self, pos: Position) -> Result<()> {
        let _: () = self
            .redis_conn
            .set("trading_bot:position", pos.as_str())
            .await?;
        Ok(())
    }

    pub async fn run_cycle(
        &mut self,
        price: f64,
        size: f64,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        info!("----Cycle start-----");
        info!("Price = {:.2} | State = {:?}", price, self.pos);

        match self.pos {
            Position::Flat => {
                if self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    info!("Entering LONG at {:.2}", price);
                    let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;
                    info!("Long executed at {:.2}", exec_price);
                    self.pos = Position::Long;
                } else if self.zones.short_zones.iter().any(|z| z.contains(price)) {
                    info!("Entering SHORT at {:.2}", price);
                    let exec_price = exchange.place_market_order(OrderSide::Sell, size).await?;
                    info!("Short executed at {:.2}", exec_price);
                    self.pos = Position::Short;
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
                }
            }

            Position::Short => {
                // 3️⃣ Cover: exit short when we hit the long zone.
                if self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    info!("Covering SHORT at {:.2}", price);
                    let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;
                    info!("Covered SHORT at {:.2}", exec_price);
                    self.pos = Position::Flat;
                }
            }
        }
        self.store_position(self.pos).await?;
        Ok(())
    }
}
