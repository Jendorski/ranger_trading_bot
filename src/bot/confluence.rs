use chrono::{DateTime, Duration, Utc};
use log::warn;
use redis::AsyncCommands;
use serde::Deserialize;

use crate::helper::{
    TRADING_BOT_GAUSSIAN_3D, TRADING_BOT_ICHIMOKU_CROSS,
    TRADING_BOT_RSI_DIV_1D, TRADING_BOT_RSI_DIV_4H,
    TRADING_BOT_RSI_REGIME, TRADING_BOT_TREND_STATE, TRADING_BOT_VRVP,
};
use crate::regime::{GaussianRegime3D, GaussianRegime3DSnapshot};
use crate::trackers::ichimoku::{IchimokuCrossSnapshot, IchimokuCrossState};
use crate::trackers::rsi_divergence_indicator::{RsiDivEvent, RsiDivSnapshot};
use crate::trackers::rsi_regime_tracker::{RegimeState, RsiRegimeSnapshot};
use crate::trackers::smart_money_concepts::{TrendDirection, TrendState};
use crate::trackers::visible_range_volume_profile::{NodeType, VrvpProfile};

// ─── Warm-up window ───────────────────────────────────────────────────────────

/// After this many seconds of runtime, absent core signals block entry rather
/// than being treated as permissive (fail-open → fail-uncertain).
pub const GATE_WARMUP_SECS: u64 = 1_800; // 30 minutes
pub const GATE_WARMUP: std::time::Duration = std::time::Duration::from_secs(GATE_WARMUP_SECS);

// ─── Staleness thresholds (2× each tracker's refresh interval) ───────────────

/// SMC loop default refresh: 1800s → warn after 3600s
const SMC_STALE_SECS: i64 = 3_600;
/// RSI regime loop refresh: 14400s → warn after 28800s
const RSI_REGIME_STALE_SECS: i64 = 28_800;
/// Ichimoku loop refresh: 14400s → warn after 28800s
const ICHIMOKU_STALE_SECS: i64 = 28_800;
/// Gaussian 3D loop refresh: 10800s → warn after 21600s
const GAUSSIAN_STALE_SECS: i64 = 21_600;
/// 4H RSI div loop refresh: 900s → warn after 1800s
const RSI_DIV_4H_STALE_SECS: i64 = 1_800;
/// 1D RSI div loop refresh: 7200s → warn after 14400s
const RSI_DIV_1D_STALE_SECS: i64 = 14_400;

// ─── Gate ─────────────────────────────────────────────────────────────────────

pub struct ConfluenceGate {
    pub trend_direction: Option<TrendDirection>,
    pub rsi_regime:      Option<RegimeState>,
    pub ichimoku_cross:  Option<IchimokuCrossState>,
    pub gaussian_3d:     Option<GaussianRegime3D>,
    pub rsi_div_4h:      Option<Vec<RsiDivEvent>>,
    pub rsi_div_1d:      Option<Vec<RsiDivEvent>>,
    /// NodeType of whichever 4H VRVP bin contains the current entry price.
    /// `None` when the VRVP profile is not yet available — treated as no-veto.
    pub vrvp_node_4h:    Option<NodeType>,
    /// True once the bot has been running for at least [`GATE_WARMUP`].
    /// After warm-up, absent core signals block entry rather than being permissive.
    past_warmup: bool,
}

impl ConfluenceGate {
    pub async fn read(
        conn: &mut redis::aio::MultiplexedConnection,
        price: f64,
        past_warmup: bool,
    ) -> Self {
        let vrvp_node_4h = {
            let key = format!("{TRADING_BOT_VRVP}:4H");
            read_json::<VrvpProfile>(conn, &key)
                .await
                .map(|p| p.node_at(price))
        };

        Self {
            trend_direction: read_json::<TrendState>(conn, TRADING_BOT_TREND_STATE)
                .await
                .inspect(|s| warn_if_stale("TrendState", s.updated_at, SMC_STALE_SECS))
                .map(|s| s.direction),

            rsi_regime: read_json::<RsiRegimeSnapshot>(conn, TRADING_BOT_RSI_REGIME)
                .await
                .inspect(|s| warn_if_stale("RsiRegime", s.updated_at, RSI_REGIME_STALE_SECS))
                .map(|s| s.regime),

            ichimoku_cross: read_json::<IchimokuCrossSnapshot>(conn, TRADING_BOT_ICHIMOKU_CROSS)
                .await
                .inspect(|s| warn_if_stale("IchimokuCross", s.updated_at, ICHIMOKU_STALE_SECS))
                .map(|s| s.state),

            gaussian_3d: read_json::<GaussianRegime3DSnapshot>(conn, TRADING_BOT_GAUSSIAN_3D)
                .await
                .inspect(|s| warn_if_stale("Gaussian3D", s.updated_at, GAUSSIAN_STALE_SECS))
                .map(|s| s.regime),

            rsi_div_4h: read_json::<RsiDivSnapshot>(conn, TRADING_BOT_RSI_DIV_4H)
                .await
                .inspect(|s| warn_if_stale("RsiDiv4H", s.updated_at, RSI_DIV_4H_STALE_SECS))
                .map(|s| s.events),

            rsi_div_1d: read_json::<RsiDivSnapshot>(conn, TRADING_BOT_RSI_DIV_1D)
                .await
                .inspect(|s| warn_if_stale("RsiDiv1D", s.updated_at, RSI_DIV_1D_STALE_SECS))
                .map(|s| s.events),

            vrvp_node_4h,
            past_warmup,
        }
    }

    pub fn permits_long(&self) -> bool {
        // After warm-up, core directional signals must all be present.
        // None means the upstream tracker has failed — treat as uncertain, block entry.
        if self.past_warmup {
            if self.trend_direction.is_none() {
                warn!("ConfluenceGate: long blocked — TrendState absent after warm-up");
                return false;
            }
            if self.rsi_regime.is_none() {
                warn!("ConfluenceGate: long blocked — RsiRegime absent after warm-up");
                return false;
            }
            if self.ichimoku_cross.is_none() {
                warn!("ConfluenceGate: long blocked — IchimokuCross absent after warm-up");
                return false;
            }
            if self.gaussian_3d.is_none() {
                warn!("ConfluenceGate: long blocked — Gaussian3D absent after warm-up");
                return false;
            }
        }

        // Veto: LVN at entry price — zone has no volume-based structural support
        if self.vrvp_node_4h == Some(NodeType::LowVolumeNode) {
            warn!("ConfluenceGate: long vetoed — 4H VRVP LVN at entry price");
            return false;
        }

        // Count-based veto: 3+ of 4 core signals simultaneously bearish
        {
            let mut bearish = 0u8;
            if self.trend_direction == Some(TrendDirection::Bearish)             { bearish += 1; }
            if self.rsi_regime      == Some(RegimeState::Bearish)                { bearish += 1; }
            if self.ichimoku_cross  == Some(IchimokuCrossState::KijunBelowSpanB) { bearish += 1; }
            if self.gaussian_3d     == Some(GaussianRegime3D::BearIntact)        { bearish += 1; }
            if bearish >= 3 {
                warn!("ConfluenceGate: long vetoed — {bearish}/4 core signals bearish");
                return false;
            }
        }

        // Veto 1: momentum + structural trend both confirmed bearish
        if self.trend_direction == Some(TrendDirection::Bearish)
            && self.rsi_regime == Some(RegimeState::Bearish)
        {
            warn!("ConfluenceGate: long vetoed — TrendState + RSI both Bearish");
            return false;
        }

        // Veto 2: both technical structure signals confirmed bearish
        if self.ichimoku_cross == Some(IchimokuCrossState::KijunBelowSpanB)
            && self.gaussian_3d == Some(GaussianRegime3D::BearIntact)
        {
            warn!("ConfluenceGate: long vetoed — Ichimoku + GC3D both Bearish");
            return false;
        }

        // Strength veto: recent bearish RSI divergence on 4H suppresses longs
        if let Some(events) = &self.rsi_div_4h {
            let has_bearish = events.iter().any(|e| {
                is_recent(e)
                    && matches!(
                        e,
                        RsiDivEvent::RegularBearish { .. } | RsiDivEvent::HiddenBearish { .. }
                    )
            });
            if has_bearish {
                warn!("ConfluenceGate: long suppressed — bearish RSI div on 4H (within {RSI_DIV_LOOKBACK_HOURS}h)");
                return false;
            }
        }

        // 1D bearish divergence: higher-TF selling pressure — independent veto
        if let Some(events) = &self.rsi_div_1d {
            let has_bearish = events.iter().any(|e| {
                is_recent(e)
                    && matches!(
                        e,
                        RsiDivEvent::RegularBearish { .. } | RsiDivEvent::HiddenBearish { .. }
                    )
            });
            if has_bearish {
                warn!("ConfluenceGate: long suppressed — bearish RSI div on 1D (within {RSI_DIV_LOOKBACK_HOURS}h)");
                return false;
            }
        }

        true
    }

    pub fn permits_short(&self) -> bool {
        // After warm-up, core directional signals must all be present.
        if self.past_warmup {
            if self.trend_direction.is_none() {
                warn!("ConfluenceGate: short blocked — TrendState absent after warm-up");
                return false;
            }
            if self.rsi_regime.is_none() {
                warn!("ConfluenceGate: short blocked — RsiRegime absent after warm-up");
                return false;
            }
            if self.ichimoku_cross.is_none() {
                warn!("ConfluenceGate: short blocked — IchimokuCross absent after warm-up");
                return false;
            }
            if self.gaussian_3d.is_none() {
                warn!("ConfluenceGate: short blocked — Gaussian3D absent after warm-up");
                return false;
            }
        }

        // Veto: LVN at entry price — zone has no volume-based structural support
        if self.vrvp_node_4h == Some(NodeType::LowVolumeNode) {
            warn!("ConfluenceGate: short vetoed — 4H VRVP LVN at entry price");
            return false;
        }

        // Count-based veto: 3+ of 4 core signals simultaneously bullish
        {
            let mut bullish = 0u8;
            if self.trend_direction == Some(TrendDirection::Bullish)             { bullish += 1; }
            if self.rsi_regime      == Some(RegimeState::Bullish)                { bullish += 1; }
            if self.ichimoku_cross  == Some(IchimokuCrossState::KijunAboveSpanB) { bullish += 1; }
            if self.gaussian_3d     == Some(GaussianRegime3D::BullIntact)        { bullish += 1; }
            if bullish >= 3 {
                warn!("ConfluenceGate: short vetoed — {bullish}/4 core signals bullish");
                return false;
            }
        }

        // Veto 1: momentum + structural trend both confirmed bullish
        if self.trend_direction == Some(TrendDirection::Bullish)
            && self.rsi_regime == Some(RegimeState::Bullish)
        {
            warn!("ConfluenceGate: short vetoed — TrendState + RSI both Bullish");
            return false;
        }

        // Veto 2: both technical structure signals confirmed bullish
        if self.ichimoku_cross == Some(IchimokuCrossState::KijunAboveSpanB)
            && self.gaussian_3d == Some(GaussianRegime3D::BullIntact)
        {
            warn!("ConfluenceGate: short vetoed — Ichimoku + GC3D both Bullish");
            return false;
        }

        // Strength veto: recent bullish RSI divergence on 4H suppresses shorts
        if let Some(events) = &self.rsi_div_4h {
            let has_bullish = events.iter().any(|e| {
                is_recent(e)
                    && matches!(
                        e,
                        RsiDivEvent::RegularBullish { .. } | RsiDivEvent::HiddenBullish { .. }
                    )
            });
            if has_bullish {
                warn!("ConfluenceGate: short suppressed — bullish RSI div on 4H (within {RSI_DIV_LOOKBACK_HOURS}h)");
                return false;
            }
        }

        // 1D bullish divergence: higher-TF buying pressure — independent veto
        if let Some(events) = &self.rsi_div_1d {
            let has_bullish = events.iter().any(|e| {
                is_recent(e)
                    && matches!(
                        e,
                        RsiDivEvent::RegularBullish { .. } | RsiDivEvent::HiddenBullish { .. }
                    )
            });
            if has_bullish {
                warn!("ConfluenceGate: short suppressed — bullish RSI div on 1D (within {RSI_DIV_LOOKBACK_HOURS}h)");
                return false;
            }
        }

        true
    }

    pub fn size_modifier_long(&self) -> f64 {
        let mut n = 0u8;
        if self.trend_direction == Some(TrendDirection::Bullish)             { n += 1; }
        if self.rsi_regime      == Some(RegimeState::Bullish)                { n += 1; }
        if self.ichimoku_cross  == Some(IchimokuCrossState::KijunAboveSpanB) { n += 1; }
        if self.gaussian_3d     == Some(GaussianRegime3D::BullIntact)        { n += 1; }
        if self.vrvp_node_4h    == Some(NodeType::HighVolumeNode)            { n += 1; }
        match n {
            4 | 5 => 1.0,
            3     => 0.75,
            2     => 0.5,
            _     => 0.25,
        }
    }

    pub fn size_modifier_short(&self) -> f64 {
        let mut n = 0u8;
        if self.trend_direction == Some(TrendDirection::Bearish)             { n += 1; }
        if self.rsi_regime      == Some(RegimeState::Bearish)                { n += 1; }
        if self.ichimoku_cross  == Some(IchimokuCrossState::KijunBelowSpanB) { n += 1; }
        if self.gaussian_3d     == Some(GaussianRegime3D::BearIntact)        { n += 1; }
        if self.vrvp_node_4h    == Some(NodeType::HighVolumeNode)            { n += 1; }
        match n {
            4 | 5 => 1.0,
            3     => 0.75,
            2     => 0.5,
            _     => 0.25,
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Only divergence events within this window are considered by the gate.
/// 10 × 4H bars = 40 hours — stale signals older than this are ignored.
const RSI_DIV_LOOKBACK_HOURS: i64 = 40;

fn warn_if_stale(label: &str, updated_at: DateTime<Utc>, max_age_secs: i64) {
    let age = Utc::now() - updated_at;
    if age > Duration::seconds(max_age_secs) {
        warn!(
            "ConfluenceGate: '{}' signal is stale — age={}s threshold={}s",
            label,
            age.num_seconds(),
            max_age_secs,
        );
    }
}

fn event_time(e: &RsiDivEvent) -> DateTime<Utc> {
    match e {
        RsiDivEvent::RegularBullish { time, .. }
        | RsiDivEvent::HiddenBullish { time, .. }
        | RsiDivEvent::RegularBearish { time, .. }
        | RsiDivEvent::HiddenBearish { time, .. } => *time,
    }
}

fn is_recent(e: &RsiDivEvent) -> bool {
    Utc::now() - event_time(e) <= Duration::hours(RSI_DIV_LOOKBACK_HOURS)
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    conn: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Option<T> {
    let raw: Option<String> = conn.get(key).await.ok()?;
    let raw = raw?;
    match serde_json::from_str::<T>(&raw) {
        Ok(v) => Some(v),
        Err(e) => {
            warn!("ConfluenceGate: failed to deserialize '{key}': {e}");
            None
        }
    }
}
