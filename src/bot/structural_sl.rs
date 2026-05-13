use chrono::{DateTime, Utc};
use log::warn;
use redis::AsyncCommands;
use serde::Deserialize;

use crate::bot::zones::Zone;
use crate::bot::Position;
use crate::helper::{TRADING_BOT_RSI_DIV_4H, TRADING_BOT_TREND_STATE, TRADING_BOT_VRVP};
use crate::trackers::rsi_divergence_indicator::{RsiDivEvent, RsiDivSnapshot};
use crate::trackers::smart_money_concepts::{TrendDirection, TrendState};
use crate::trackers::visible_range_volume_profile::{NodeType, VrvpNode, VrvpProfile};

// ─── Types ───────────────────────────────────────────────────────────────────

pub enum SlTightenResult {
    NoChange,
    Tighten(f64),
    /// 4H BOS reversed against the trade direction — exit immediately.
    ExitNow,
}

// ─── Initial SL placement ────────────────────────────────────────────────────

/// Returns the structurally-anchored initial SL price for the triggering zone.
///
/// Long:  SL = `zone.low  − (zone_width × buffer_multiplier)`
/// Short: SL = `zone.high + (zone_width × buffer_multiplier)`
///
/// Position sizing is the caller's responsibility: use `Helper::risk_anchored_qty` with
/// this price so that the SL distance determines the quantity, not leverage × margin.
pub fn compute_initial_sl(pos: Position, zone: &Zone, buffer_multiplier: f64) -> f64 {
    let zone_width = zone.high - zone.low;
    let buffer = zone_width * buffer_multiplier;
    match pos {
        Position::Long  => {
            let sl = zone.low - buffer;
            warn!("StructuralSL (long):  zone [{:.2}–{:.2}] buffer={:.2} → sl={sl:.2}", zone.low, zone.high, buffer);
            sl
        }
        Position::Short => {
            let sl = zone.high + buffer;
            warn!("StructuralSL (short): zone [{:.2}–{:.2}] buffer={:.2} → sl={sl:.2}", zone.low, zone.high, buffer);
            sl
        }
        Position::Flat  => 0.0,
    }
}

// ─── Dynamic SL tightening ───────────────────────────────────────────────────

/// Evaluates whether the SL should be tightened on the current price poll.
///
/// Checks three signals in priority order:
/// 1. 4H BOS reversed against trade direction → `ExitNow` (structural basis gone)
/// 2. RSI bullish divergence on 4H while short (or bearish while long) → tighten to nearest HVN
/// 3. Price adjacent to a VRVP 4H HVN opposing the trade → tighten to HVN boundary
///
/// Only ever tightens — never widens. Returns `NoChange` if no signal fires.
pub async fn evaluate_sl_tighten(
    conn: &mut redis::aio::MultiplexedConnection,
    current_price: f64,
    current_sl: f64,
    entry_time: DateTime<Utc>,
    pos: Position,
) -> SlTightenResult {
    // Priority 1 — structural reversal: BOS flipped after entry
    if let Some(trend) = read_json::<TrendState>(conn, TRADING_BOT_TREND_STATE).await {
        let bos_after_entry = trend
            .last_bos_time
            .map(|t| t > entry_time)
            .unwrap_or(false);

        if bos_after_entry {
            match (pos, trend.direction) {
                (Position::Short, TrendDirection::Bullish) => {
                    warn!("StructuralSL: 4H BullishBOS after entry while short — exit now");
                    return SlTightenResult::ExitNow;
                }
                (Position::Long, TrendDirection::Bearish) => {
                    warn!("StructuralSL: 4H BearishBOS after entry while long — exit now");
                    return SlTightenResult::ExitNow;
                }
                _ => {}
            }
        }
    }

    // Priority 2 — RSI divergence opposing trade direction, post-entry only.
    // Only divergence that formed AFTER entry is relevant: pre-entry signals were
    // already evaluated by the confluence gate at the time of entry and either
    // blocked it or were accepted. Acting on them again inside the position would
    // tighten the SL based on stale momentum data.
    if let Some(snapshot) = read_json::<RsiDivSnapshot>(conn, TRADING_BOT_RSI_DIV_4H).await {
        let has_opposing_div = match pos {
            Position::Short => snapshot.events.iter().any(|e| {
                e.time() > entry_time
                    && matches!(
                        e,
                        RsiDivEvent::RegularBullish { .. } | RsiDivEvent::HiddenBullish { .. }
                    )
            }),
            Position::Long => snapshot.events.iter().any(|e| {
                e.time() > entry_time
                    && matches!(
                        e,
                        RsiDivEvent::RegularBearish { .. } | RsiDivEvent::HiddenBearish { .. }
                    )
            }),
            Position::Flat => false,
        };

        if has_opposing_div {
            if let Some(new_sl) = hvn_tighten_sl(conn, current_price, current_sl, pos).await {
                warn!("StructuralSL: RSI div signal → tighten SL to {new_sl:.2}");
                return SlTightenResult::Tighten(new_sl);
            }
        }
    }

    // Priority 3 — price adjacent to a VRVP HVN opposing the trade
    if let Some(new_sl) = hvn_tighten_sl(conn, current_price, current_sl, pos).await {
        warn!("StructuralSL: VRVP HVN proximity → tighten SL to {new_sl:.2}");
        return SlTightenResult::Tighten(new_sl);
    }

    SlTightenResult::NoChange
}

// ─── VRVP helpers ────────────────────────────────────────────────────────────

/// Returns a tightened SL price if the current price is within one bin width of an HVN
/// that opposes the trade direction. Returns `None` if no applicable HVN is close enough
/// or if the candidate SL would widen rather than tighten.
async fn hvn_tighten_sl(
    conn: &mut redis::aio::MultiplexedConnection,
    current_price: f64,
    current_sl: f64,
    pos: Position,
) -> Option<f64> {
    let key = format!("{TRADING_BOT_VRVP}:4H");
    let profile = read_json::<VrvpProfile>(conn, &key).await?;

    let bin_width = profile
        .bins
        .first()
        .map(|b| b.price_high - b.price_low)
        .unwrap_or(0.0);

    if bin_width == 0.0 {
        return None;
    }

    match pos {
        Position::Short => {
            let hvn = nearest_hvn_above_price(&profile, current_price)?;
            if hvn.bin.price_low - current_price > bin_width {
                return None;
            }
            let candidate = hvn.bin.price_high + (bin_width * 0.25);
            if candidate < current_sl {
                Some(candidate)
            } else {
                None
            }
        }
        Position::Long => {
            let hvn = nearest_hvn_below_price(&profile, current_price)?;
            if current_price - hvn.bin.price_high > bin_width {
                return None;
            }
            let candidate = hvn.bin.price_low - (bin_width * 0.25);
            if candidate > current_sl {
                Some(candidate)
            } else {
                None
            }
        }
        Position::Flat => None,
    }
}

pub fn nearest_hvn_above_price(profile: &VrvpProfile, price: f64) -> Option<&VrvpNode> {
    profile
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::HighVolumeNode && n.bin.price_low >= price)
        .min_by(|a, b| {
            a.bin
                .price_low
                .partial_cmp(&b.bin.price_low)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

pub fn nearest_hvn_below_price(profile: &VrvpProfile, price: f64) -> Option<&VrvpNode> {
    profile
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::HighVolumeNode && n.bin.price_high <= price)
        .max_by(|a, b| {
            a.bin
                .price_high
                .partial_cmp(&b.bin.price_high)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

// ─── Private ─────────────────────────────────────────────────────────────────

async fn read_json<T: for<'de> Deserialize<'de>>(
    conn: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Option<T> {
    let raw: Option<String> = conn.get(key).await.ok()?;
    serde_json::from_str::<T>(&raw?).ok()
}
