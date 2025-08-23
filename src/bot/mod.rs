use anyhow::Result;
use log::{info, warn};

use crate::exchange::{Exchange, OrderSide};
//use std::sync::Arc;

/// Price band constants (in USD)
// const BUY_ZONE_LOW: f64 = 115_200.0;
// const BUY_ZONE_HIGH: f64 = 115_500.0;

// const SELL_ZONE_LOW: f64 = 119_900.0;
// const SELL_ZONE_HIGH: f64 = 120_008.0;

#[derive(Debug, Clone, Copy)]
pub struct Zone {
    pub low: f64,
    pub high: f64,
}

impl Zone {
    /// Returns true if price lies in the zone
    #[inline]
    pub fn contains(&self, price: f64) -> bool {
        price <= self.low && price >= self.high
    }
}

#[derive(Debug)]
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
                    low: 114_684.1,
                    high: 115_121.6,
                },
                Zone {
                    low: 111_625.9,
                    high: 112_064.8,
                },
                Zone {
                    low: 115_900.0,
                    high: 116_000.0,
                },
            ],
            short_zones: vec![
                Zone {
                    low: 123_000.0,
                    high: 122_900.0,
                },
                Zone {
                    low: 118_900.0,
                    high: 119_000.0,
                },
                Zone {
                    low: 116_788.0,
                    high: 117_427.0,
                },
                Zone {
                    low: 114_311.0,
                    high: 114_490.0,
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

/// Trading state – we keep track of whether we have an open position
#[derive(Debug)]
pub struct Bot {
    /// None if no position; Some(OrderSide::Buy) means long, Sell → short
    pub pos: Position, //Option<OrderSide>,

    pub zones: Zones,
}

impl Bot {
    pub fn new() -> Self {
        Self {
            pos: Position::Flat,
            zones: Zones::default(),
        }
    }

    pub async fn run_cycle(
        &mut self,
        price: f64,
        size: f64,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        warn!("What is the current position at?: {:#?}", self.pos);

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
        Ok(())
    }
}

// impl Bot {
//     pub fn new() -> Self {
//         Self {
//             pos: Position::Flat,
//             zones: Zones::default(),
//         }
//     }

//     /// Main decision routine – called every poll cycle.
//     ///
//     /// * `price`   – current spot price
//     /// * `order_size` – how many units to trade
//     /// * `exchange`  – the exchange implementation
//     pub async fn run_cycle(
//         &mut self,
//         price: f64,
//         size: f64,
//         exchange: Arc<dyn Exchange>,
//     ) -> Result<()> {
//         // 1️⃣ No open position → decide whether to enter a trade
//         if self.pos.is_none() {
//             if price >= BUY_ZONE_LOW && price <= BUY_ZONE_HIGH {
//                 info!(
//                     "Price {:.2} in BUY zone – opening LONG for {} BTC",
//                     price, size
//                 );
//                 let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;
//                 self.pos = Some(OrderSide::Buy);
//                 info!("Executed at {:.2}", exec_price);
//             } else if price >= SELL_ZONE_LOW && price <= SELL_ZONE_HIGH {
//                 info!(
//                     "Price {:.2} in SHORT zone – opening SHORT for {} BTC",
//                     price, size
//                 );
//                 let exec_price = exchange.place_market_order(OrderSide::Sell, size).await?;
//                 self.pos = Some(OrderSide::Sell);
//                 info!("Executed at {:.2}", exec_price);
//             } else {
//                 // no action
//                 warn!("Price {:.2} out of any zone – staying flat", price);
//             }
//         }

//         // 2️⃣ Position open → check for take‑profit
//         if let Some(side) = self.pos.clone() {
//             match side {
//                 OrderSide::Buy => {
//                     // Long: close when price enters SELL zone
//                     if price >= SELL_ZONE_LOW && price <= SELL_ZONE_HIGH {
//                         info!("Price {:.2} in TAKE‑PROFIT zone for LONG – closing", price);
//                         let exec_price = exchange.place_market_order(OrderSide::Sell, size).await?;
//                         self.pos = None;
//                         info!("Closed at {:.2}", exec_price);
//                     }
//                 }
//                 OrderSide::Sell => {
//                     // Short: close when price enters BUY zone
//                     if price >= BUY_ZONE_LOW && price <= BUY_ZONE_HIGH {
//                         info!(
//                             "Price {:.2} in TAKE‑PROFIT zone for SHORT – covering",
//                             price
//                         );
//                         let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;
//                         self.pos = None;
//                         info!("Covered at {:.2}", exec_price);
//                     }
//                 }
//             }
//         }

//         Ok(())
//     }
// }
