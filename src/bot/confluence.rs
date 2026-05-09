use log::warn;
use redis::AsyncCommands;
use serde::Deserialize;

use crate::helper::{
    TRADING_BOT_GAUSSIAN_3D, TRADING_BOT_ICHIMOKU_CROSS,
    TRADING_BOT_RSI_DIV_1D, TRADING_BOT_RSI_DIV_4H,
    TRADING_BOT_RSI_REGIME, TRADING_BOT_TREND_STATE,
};
use crate::regime::{GaussianRegime3D, GaussianRegime3DSnapshot};
use crate::trackers::ichimoku::{IchimokuCrossSnapshot, IchimokuCrossState};
use crate::trackers::rsi_divergence_indicator::{RsiDivEvent, RsiDivSnapshot};
use crate::trackers::rsi_regime_tracker::{RegimeState, RsiRegimeSnapshot};
use crate::trackers::smart_money_concepts::{TrendDirection, TrendState};

pub struct ConfluenceGate {
    pub trend_direction: Option<TrendDirection>,
    pub rsi_regime:      Option<RegimeState>,
    pub ichimoku_cross:  Option<IchimokuCrossState>,
    pub gaussian_3d:     Option<GaussianRegime3D>,
    pub rsi_div_4h:      Option<Vec<RsiDivEvent>>,
    pub rsi_div_1d:      Option<Vec<RsiDivEvent>>,
}

impl ConfluenceGate {
    pub async fn read(conn: &mut redis::aio::MultiplexedConnection) -> Self {
        Self {
            trend_direction: read_json::<TrendState>(conn, TRADING_BOT_TREND_STATE)
                .await
                .map(|s| s.direction),
            rsi_regime: read_json::<RsiRegimeSnapshot>(conn, TRADING_BOT_RSI_REGIME)
                .await
                .map(|s| s.regime),
            ichimoku_cross: read_json::<IchimokuCrossSnapshot>(conn, TRADING_BOT_ICHIMOKU_CROSS)
                .await
                .map(|s| s.state),
            gaussian_3d: read_json::<GaussianRegime3DSnapshot>(conn, TRADING_BOT_GAUSSIAN_3D)
                .await
                .map(|s| s.regime),
            rsi_div_4h: read_json::<RsiDivSnapshot>(conn, TRADING_BOT_RSI_DIV_4H)
                .await
                .map(|s| s.events),
            rsi_div_1d: read_json::<RsiDivSnapshot>(conn, TRADING_BOT_RSI_DIV_1D)
                .await
                .map(|s| s.events),
        }
    }

    pub fn permits_long(&self) -> bool {
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
                matches!(
                    e,
                    RsiDivEvent::RegularBearish { .. } | RsiDivEvent::HiddenBearish { .. }
                )
            });
            if has_bearish {
                warn!("ConfluenceGate: long suppressed — bearish RSI div on 4H");
                return false;
            }
        }

        // 1D bearish divergence: higher-TF selling pressure — independent veto
        if let Some(events) = &self.rsi_div_1d {
            let has_bearish = events.iter().any(|e| {
                matches!(
                    e,
                    RsiDivEvent::RegularBearish { .. } | RsiDivEvent::HiddenBearish { .. }
                )
            });
            if has_bearish {
                warn!("ConfluenceGate: long suppressed — bearish RSI div on 1D");
                return false;
            }
        }

        true
    }

    pub fn permits_short(&self) -> bool {
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
                matches!(
                    e,
                    RsiDivEvent::RegularBullish { .. } | RsiDivEvent::HiddenBullish { .. }
                )
            });
            if has_bullish {
                warn!("ConfluenceGate: short suppressed — bullish RSI div on 4H");
                return false;
            }
        }

        // 1D bullish divergence: higher-TF buying pressure — independent veto
        if let Some(events) = &self.rsi_div_1d {
            let has_bullish = events.iter().any(|e| {
                matches!(
                    e,
                    RsiDivEvent::RegularBullish { .. } | RsiDivEvent::HiddenBullish { .. }
                )
            });
            if has_bullish {
                warn!("ConfluenceGate: short suppressed — bullish RSI div on 1D");
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
        match n {
            3 | 4 => 1.0,
            2     => 0.75,
            1     => 0.5,
            _     => 0.25,
        }
    }

    pub fn size_modifier_short(&self) -> f64 {
        let mut n = 0u8;
        if self.trend_direction == Some(TrendDirection::Bearish)             { n += 1; }
        if self.rsi_regime      == Some(RegimeState::Bearish)                { n += 1; }
        if self.ichimoku_cross  == Some(IchimokuCrossState::KijunBelowSpanB) { n += 1; }
        if self.gaussian_3d     == Some(GaussianRegime3D::BearIntact)        { n += 1; }
        match n {
            3 | 4 => 1.0,
            2     => 0.75,
            1     => 0.5,
            _     => 0.25,
        }
    }
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
