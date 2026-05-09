use log::info;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::hash::Hasher;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
        let distance = (self.midpoint() - other.midpoint()).abs();
        distance < min_distance
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Zones {
    pub long_zones: Vec<Zone>,
    pub short_zones: Vec<Zone>,
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

/// How long a cached ZoneStats read is considered fresh before re-fetching Redis.
const ZONE_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ZoneStats {
    pub consecutive_losses: u8,
    pub disabled: bool,
    pub cooldown_until: Option<u64>, // unix timestamp
}

#[derive(Debug)]
pub struct ZoneGuard {
    /// In-memory cache: zone stats paired with the instant they were last fetched.
    zones: HashMap<ZoneId, (ZoneStats, Instant)>,
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

    #[allow(dead_code)]
    pub fn can_trade(&self, zone_id: ZoneId) -> bool {
        self.zones
            .get(&zone_id)
            .map(|(stats, _)| !stats.disabled)
            .unwrap_or(true)
    }

    /// Returns the `ZoneStats` for `zone_id`, serving from the in-memory cache
    /// if the entry is younger than [`ZONE_CACHE_TTL`]. Falls back to Redis on a
    /// cache miss or expiry and refreshes the cache entry.
    pub async fn get_trade_result(&mut self, zone_id: ZoneId) -> ZoneStats {
        if let Some((stats, cached_at)) = self.zones.get(&zone_id) {
            if cached_at.elapsed() < ZONE_CACHE_TTL {
                return stats.clone();
            }
        }

        let key = format!("zone_stats::{}", zone_id.0);
        let raw: String = self
            .redis_conn
            .get(&key)
            .await
            .unwrap_or_else(|_| "{}".to_string());
        let stats: ZoneStats = serde_json::from_str(&raw).unwrap_or_default();

        self.zones.insert(zone_id, (stats.clone(), Instant::now()));
        stats
    }

    pub async fn record_trade_result(&mut self, zone_id: ZoneId, pnl: f64) {
        let (stats, cached_at) = self
            .zones
            .entry(zone_id)
            .or_insert_with(|| (ZoneStats::default(), Instant::now()));

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

        // Refresh cache timestamp so the next tick reads the updated value locally.
        *cached_at = Instant::now();

        let zone_expiry = stats
            .cooldown_until
            .map(|ts| ts.saturating_sub(Self::now()))
            .unwrap_or(60 * 60 * 6)
            .try_into()
            .unwrap();
        info!("Zone expiry: {zone_expiry}");

        let _: () = self
            .redis_conn
            .set_ex(
                format!("zone_stats::{}", zone_id.0),
                serde_json::to_string(stats).unwrap(),
                zone_expiry,
            )
            .await
            .unwrap();
    }
}
