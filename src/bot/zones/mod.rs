use log::info;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::hash::Hasher;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{collections::HashMap, hash::Hash};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Side {
    Long,
    Short,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Zone {
    pub low: f64,
    pub high: f64,
    pub side: Side,
}

impl Zone {
    /// Returns true if price lies in the zone
    #[inline]
    pub fn contains(&self, price: f64) -> bool {
        price >= self.low && price <= self.high
    }

    #[inline]
    pub fn midpoint(&self) -> f64 {
        (self.low + self.high) / 2.0
    }

    #[inline]
    pub fn overlaps_or_too_close(&self, other: &Zone, min_distance: f64) -> bool {
        // Check if zones overlap or are closer than min_distance
        let distance = (self.midpoint() - other.midpoint()).abs();
        distance < min_distance
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
                    side: Side::Long,
                },
                Zone {
                    low: 79_981.80,
                    high: 80_102.80,
                    side: Side::Long,
                },
                Zone {
                    low: 83_991.80,
                    high: 84_092.80,
                    side: Side::Long,
                },
                Zone {
                    low: 86_401.80,
                    high: 86_602.80,
                    side: Side::Long,
                },
                Zone {
                    low: 109_018.9,
                    high: 109_122.8,
                    side: Side::Long,
                },
                Zone {
                    low: 113_293.9,
                    high: 113_393.8,
                    side: Side::Long,
                },
                Zone {
                    low: 114_548.9,
                    high: 114_677.8,
                    side: Side::Long,
                },
                Zone {
                    low: 116_344.4,
                    high: 116_464.4,
                    side: Side::Long,
                },
                Zone {
                    low: 121_100.0,
                    high: 121_350.0,
                    side: Side::Long,
                },
                Zone {
                    low: 122_400.0,
                    high: 122_350.0,
                    side: Side::Long,
                },
                Zone {
                    low: 123_100.0,
                    high: 123_150.0,
                    side: Side::Long,
                },
                Zone {
                    low: 124_600.0,
                    high: 124_650.0,
                    side: Side::Long,
                },
                Zone {
                    low: 124_199.0,
                    high: 125_220.0,
                    side: Side::Long,
                },
            ],
            short_zones: vec![
                Zone {
                    low: 125_797.0,
                    high: 125_897.0,
                    side: Side::Short,
                },
                Zone {
                    low: 125_097.0,
                    high: 125_197.0,
                    side: Side::Short,
                },
                Zone {
                    low: 124_500.0,
                    high: 124_540.0,
                    side: Side::Short,
                },
                Zone {
                    low: 123_990.0,
                    high: 124_032.0,
                    side: Side::Short,
                },
                Zone {
                    low: 122_900.0,
                    high: 123_000.0,
                    side: Side::Short,
                },
                Zone {
                    low: 120_931.4,
                    high: 120_170.1,
                    side: Side::Short,
                },
                Zone {
                    low: 119_409.0,
                    high: 119_479.7,
                    side: Side::Short,
                },
                Zone {
                    low: 117_514.0,
                    high: 118_008.3,
                    side: Side::Short,
                },
                Zone {
                    low: 115_585.0,
                    high: 116_085.2,
                    side: Side::Short,
                },
                Zone {
                    low: 114_316.0,
                    high: 114_486.2,
                    side: Side::Short,
                },
                Zone {
                    low: 112_990.0,
                    high: 113_100.0,
                    side: Side::Short,
                },
                Zone {
                    low: 108_511.0,
                    high: 108_646.0,
                    side: Side::Short,
                },
                Zone {
                    low: 104_511.00,
                    high: 104_596.30,
                    side: Side::Short,
                },
                Zone {
                    low: 98_030.10,
                    high: 98_079.60,
                    side: Side::Short,
                },
                Zone {
                    low: 93_930.10,
                    high: 94_079.60,
                    side: Side::Short,
                },
                Zone {
                    low: 92_630.10,
                    high: 92_679.60,
                    side: Side::Short,
                },
                Zone {
                    low: 89_906.80,
                    high: 90_008.60,
                    side: Side::Short,
                },
                Zone {
                    low: 73_906.80,
                    high: 73_979.60,
                    side: Side::Short,
                },
            ],
        }
    }
}

/* =======================
   ZoneId (Stable)
======================= */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ZoneId(u64);

impl ZoneId {
    pub fn from_zone(zone: &Zone) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        match zone.side {
            Side::Long => 1u8.hash(&mut hasher),
            Side::Short => 2u8.hash(&mut hasher),
        }

        zone.low.to_bits().hash(&mut hasher);
        zone.high.to_bits().hash(&mut hasher);

        ZoneId(hasher.finish())
    }
}

/* =======================
   Zone Guard
======================= */

#[derive(Debug, Serialize, Deserialize)]
pub struct ZoneStats {
    pub consecutive_losses: u8,
    pub disabled: bool,
    pub cooldown_until: Option<u64>, // unix timestamp
}

impl Default for ZoneStats {
    fn default() -> Self {
        Self {
            consecutive_losses: 0,
            disabled: false,
            cooldown_until: None,
        }
    }
}

#[derive(Debug)]
pub struct ZoneGuard {
    zones: HashMap<ZoneId, ZoneStats>,
    max_losses: u8,
    redis_conn: redis::aio::MultiplexedConnection,
    cooldown_secs: u64,
}

impl ZoneGuard {
    pub fn new(
        max_losses: u8,
        conn: redis::aio::MultiplexedConnection,
        cooldown_secs: u64,
    ) -> Self {
        Self {
            zones: HashMap::new(),
            max_losses,
            redis_conn: conn,
            cooldown_secs,
        }
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    pub fn can_trade(&self, zone_id: ZoneId) -> bool {
        self.zones
            .get(&zone_id)
            .map(|z| !z.disabled)
            .unwrap_or(true)
    }

    pub async fn get_trade_result(&mut self, zone_id: ZoneId) -> ZoneStats {
        let key: String = format!("zone_stats::{}", zone_id.0);
        let stats: String = self.redis_conn.get(key).await.unwrap_or(String::from("{}"));
        let stats: ZoneStats = serde_json::from_str(&stats).unwrap_or(ZoneStats {
            consecutive_losses: 0,
            disabled: false,
            cooldown_until: None,
        });

        return stats;
    }

    pub async fn record_trade_result(&mut self, zone_id: ZoneId, pnl: f64) {
        let stats = self.zones.entry(zone_id).or_default();

        if pnl < 0.0 {
            stats.consecutive_losses += 1;

            if stats.consecutive_losses >= self.max_losses {
                stats.disabled = true;
                info!(
                    "Self::now() + self.cooldown_secs): {:?}",
                    Self::now() + self.cooldown_secs
                );
                stats.cooldown_until = Some(Self::now() + self.cooldown_secs);
            }
        } else {
            stats.consecutive_losses = 0;
        }
        let _: () = self
            .redis_conn
            .set_ex(
                format!("zone_stats::{}", zone_id.0),
                serde_json::to_string(&stats).unwrap(),
                stats
                    .cooldown_until
                    .unwrap_or(60 * 60 * 12)
                    .try_into()
                    .unwrap(),
            )
            .await
            .unwrap();
    }
}
