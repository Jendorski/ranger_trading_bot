use log::info;
use redis::AsyncCommands;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;

use crate::bot::zones::Zones;
use crate::bot::Position;
use crate::helper::{PartialProfitTarget, TRADING_BOT_VRVP, TRADING_BOT_ZONES};
use crate::trackers::visible_range_volume_profile::{NodeType, VrvpProfile};

#[derive(Debug, Clone)]
pub enum TpSource {
    SmcZone,
    VrvpHvn { timeframe: String },
}

#[derive(Debug, Clone)]
pub struct StructuralTpLevel {
    pub price: f64,
    pub source: TpSource,
    pub distance_from_entry: f64,
}

/// Collects up to `max_levels` structural TP candidates from SMC zones and VRVP HVNs.
///
/// For a short: candidates are levels BELOW entry (where buyers are expected to defend).
/// For a long:  candidates are levels ABOVE entry (where sellers are expected to push back).
///
/// SMC zones take priority over VRVP HVNs at the same price level. If an HVN falls within
/// `min_distance` of an SMC zone, the HVN is dropped and the coincidence is logged as
/// double confirmation.
pub async fn collect_structural_tp_levels(
    conn: &mut redis::aio::MultiplexedConnection,
    entry_price: f64,
    pos: Position,
    min_distance: f64,
    max_levels: usize,
) -> Vec<StructuralTpLevel> {
    let mut candidates: Vec<StructuralTpLevel> = Vec::new();

    // --- SMC zones ---
    if let Some(zones) = read_json::<Zones>(conn, TRADING_BOT_ZONES).await {
        let prices: Vec<f64> = match pos {
            Position::Short => zones
                .long_zones
                .into_iter()
                .filter(|z| z.high < entry_price)
                .map(|z| z.high)
                .collect(),
            Position::Long => zones
                .short_zones
                .into_iter()
                .filter(|z| z.low > entry_price)
                .map(|z| z.low)
                .collect(),
            Position::Flat => vec![],
        };
        for price in prices {
            candidates.push(StructuralTpLevel {
                price,
                source: TpSource::SmcZone,
                distance_from_entry: (entry_price - price).abs(),
            });
        }
    }

    // --- VRVP HVNs: 4H (nearby structure) then 1D (farther targets) ---
    for tf in &["4H", "1D"] {
        let key = format!("{TRADING_BOT_VRVP}:{tf}");
        if let Some(profile) = read_json::<VrvpProfile>(conn, &key).await {
            for node in profile
                .nodes
                .iter()
                .filter(|n| n.node_type == NodeType::HighVolumeNode)
            {
                let price = node.bin.price_mid;
                let qualifies = match pos {
                    Position::Short => price < entry_price,
                    Position::Long => price > entry_price,
                    Position::Flat => false,
                };
                if qualifies {
                    candidates.push(StructuralTpLevel {
                        price,
                        source: TpSource::VrvpHvn {
                            timeframe: tf.to_string(),
                        },
                        distance_from_entry: (entry_price - price).abs(),
                    });
                }
            }
        }
    }

    if candidates.is_empty() {
        return candidates;
    }

    // Sort nearest-first; at equal distance SmcZone wins over VrvpHvn
    candidates.sort_by(|a, b| {
        a.distance_from_entry
            .partial_cmp(&b.distance_from_entry)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| match (&a.source, &b.source) {
                (TpSource::SmcZone, TpSource::VrvpHvn { .. }) => std::cmp::Ordering::Less,
                (TpSource::VrvpHvn { .. }, TpSource::SmcZone) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            })
    });

    // Dedup: drop candidates within min_distance of an already-kept level.
    // Dropped VrvpHvns that are close to an SmcZone are logged as double confirmation.
    let mut kept: Vec<StructuralTpLevel> = Vec::with_capacity(max_levels);
    for candidate in candidates {
        let too_close = kept
            .iter()
            .any(|k| (candidate.price - k.price).abs() < min_distance);

        if too_close {
            if matches!(candidate.source, TpSource::VrvpHvn { .. }) {
                info!(
                    "StructuralTP: double confirmation — VRVP HVN @ {:.2} overlaps SMC zone",
                    candidate.price
                );
            }
            continue;
        }

        kept.push(candidate);
        if kept.len() == max_levels {
            break;
        }
    }

    kept
}

/// Builds a `PartialProfitTarget` ladder using structural levels as TP prices.
///
/// The fraction ladder and SL-stepping logic are identical to `Helper::build_profit_targets`.
/// If fewer than 4 structural levels are provided, the remaining TPs are filled with
/// arithmetic steps of `fallback_step` from the last structural level.
pub fn build_profit_targets_structural(
    levels: Vec<StructuralTpLevel>,
    entry_price: Decimal,
    margin: Decimal,
    leverage: Decimal,
    pos: Position,
    fallback_step: Decimal,
) -> Vec<PartialProfitTarget> {
    let fractions: &[Decimal] = &[dec!(0.20), dec!(0.30), dec!(0.30), dec!(0.20)];
    let size_precision: u32 = 5;
    let tp_count: usize = 4;

    // Build the TP price list: structural levels first, then arithmetic padding
    let mut tp_prices: Vec<Decimal> = levels
        .iter()
        .map(|l| Decimal::from_f64(l.price).unwrap_or(dec!(0)))
        .collect();

    while tp_prices.len() < tp_count {
        let base = tp_prices.last().copied().unwrap_or(entry_price);
        let next = match pos {
            Position::Short => base - fallback_step,
            Position::Long => base + fallback_step,
            Position::Flat => break,
        };
        tp_prices.push(next);
    }

    // Log each level with its source
    for (i, level) in levels.iter().enumerate() {
        let label = match &level.source {
            TpSource::SmcZone => "SMC zone".to_string(),
            TpSource::VrvpHvn { timeframe } => format!("VRVP {timeframe} HVN"),
        };
        info!("TP{} @ {:.2} ← {}", i + 1, level.price, label);
    }
    for (i, price) in tp_prices.iter().enumerate().skip(levels.len()) {
        info!("TP{} @ {price} ← arithmetic fallback", i + 1);
    }

    // Position size in BTC
    let notional = margin * leverage;
    let total_size = if entry_price.is_zero() {
        dec!(0)
    } else {
        (notional / entry_price).round_dp(size_precision)
    };

    let mut remaining = total_size;
    let mut ladder = Vec::with_capacity(tp_prices.len());

    for i in 0..tp_prices.len() {
        let is_last = i == tp_prices.len() - 1;

        let size = if is_last {
            remaining
        } else {
            let raw = (total_size * fractions[i]).round_dp_with_strategy(
                size_precision,
                rust_decimal::RoundingStrategy::ToZero,
            );
            remaining -= raw;
            raw
        };

        // SL stepping: after TP1 move SL to entry (break-even);
        // after each subsequent TP move SL to the previous TP price.
        let next_sl = if is_last {
            None
        } else if i == 0 {
            Some(entry_price)
        } else {
            Some(tp_prices[i - 1])
        };

        ladder.push(PartialProfitTarget {
            target_price: tp_prices[i],
            fraction: fractions[i],
            size_btc: size,
            sl: next_sl,
        });
    }

    ladder
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    conn: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Option<T> {
    let raw: Option<String> = conn.get(key).await.ok()?;
    serde_json::from_str::<T>(&raw?).ok()
}
