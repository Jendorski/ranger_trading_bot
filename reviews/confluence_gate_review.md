# 8th May 2026 - Confluence gate setup

---

## Task 3 — `ConfluenceGate` Implementation Plan

---

### Pre-requisite gap

`RsiDivSnapshot`, the RSI div Redis constants, and the `rsi_div_loop` spawn were reverted from a previous session. The Strength pillar (4H RSI divergence) cannot be read from Redis until they are restored. **Step 0 must be completed before the gate file is written.**

---

### Step 0 — Restore RSI divergence Redis infrastructure

**`src/helper/mod.rs`** — add after `TRADING_BOT_GAUSSIAN_3D`:

```rust
pub const TRADING_BOT_RSI_DIV_4H: &str = "trading_bot:rsi_div:4H";
pub const TRADING_BOT_RSI_DIV_1D: &str = "trading_bot:rsi_div:1D";
```

**`src/trackers/rsi_divergence_indicator/mod.rs`** — add `RsiDivSnapshot`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsiDivSnapshot {
    pub timeframe: String,
    pub events: Vec<RsiDivEvent>,
    pub updated_at: DateTime<Utc>,
}
```

Add `rsi_div_loop` (public) and `rsi_div_main` (private) following the same seed+delta pattern as every other tracker: seed warmup → `RsiDivEngine` clone per tick → fetch live candles → replay delta → collect all events → write `RsiDivSnapshot` to Redis.

**`src/tasks/mod.rs`** — spawn:

```rust
// 4H RSI divergence — Strength gate; 300 candles; refresh every 15 minutes
// 1D RSI divergence — higher-TF confirmation; 200 candles; refresh every 2 hours
```

`seed_4h` and `seed_1d` are already loaded.

---

### Step 1 — Create `src/bot/confluence.rs`

#### Imports

```rust
use log::warn;
use redis::AsyncCommands;
use serde::Deserialize;

use crate::helper::{
    TRADING_BOT_GAUSSIAN_3D, TRADING_BOT_ICHIMOKU_CROSS,
    TRADING_BOT_RSI_DIV_4H, TRADING_BOT_RSI_REGIME, TRADING_BOT_TREND_STATE,
};
use crate::regime::{GaussianRegime3D, GaussianRegime3DSnapshot};
use crate::trackers::ichimoku::{IchimokuCrossSnapshot, IchimokuCrossState};
use crate::trackers::rsi_divergence_indicator::{RsiDivEvent, RsiDivSnapshot};
use crate::trackers::rsi_regime_tracker::{RegimeState, RsiRegimeSnapshot};
use crate::trackers::smart_money_concepts::{TrendDirection, TrendState};
```

#### Struct

```rust
pub struct ConfluenceGate {
    pub trend_direction: Option<TrendDirection>,     // trading_bot:trend_state
    pub rsi_regime:      Option<RegimeState>,        // trading_bot:rsi_regime
    pub ichimoku_cross:  Option<IchimokuCrossState>, // trading_bot:ichimoku_cross
    pub gaussian_3d:     Option<GaussianRegime3D>,   // trading_bot:gaussian_regime_3d
    pub rsi_div_4h:      Option<Vec<RsiDivEvent>>,   // trading_bot:rsi_div:4H
}
```

#### `read` — async constructor

Each key is read independently. Missing key or deserialisation failure → `None` + `warn!`. Gate continues with remaining signals (fail-open).

```rust
impl ConfluenceGate {
    pub async fn read(conn: &mut redis::aio::MultiplexedConnection) -> Self {
        Self {
            trend_direction: read_json::<TrendState>(conn, TRADING_BOT_TREND_STATE)
                .await.map(|s| s.direction),
            rsi_regime: read_json::<RsiRegimeSnapshot>(conn, TRADING_BOT_RSI_REGIME)
                .await.map(|s| s.regime),
            ichimoku_cross: read_json::<IchimokuCrossSnapshot>(conn, TRADING_BOT_ICHIMOKU_CROSS)
                .await.map(|s| s.state),
            gaussian_3d: read_json::<GaussianRegime3DSnapshot>(conn, TRADING_BOT_GAUSSIAN_3D)
                .await.map(|s| s.regime),
            rsi_div_4h: read_json::<RsiDivSnapshot>(conn, TRADING_BOT_RSI_DIV_4H)
                .await.map(|s| s.events),
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
```

#### `permits_long`

Two hard veto conditions — each requires **two confirming bearish signals**. A single bearish signal alone never blocks; it reduces size. `None` on any signal never contributes to a veto.

```rust
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
            matches!(e, RsiDivEvent::RegularBearish { .. } | RsiDivEvent::HiddenBearish { .. })
        });
        if has_bearish {
            warn!("ConfluenceGate: long suppressed — bearish RSI div on 4H");
            return false;
        }
    }

    true
}
```

#### `permits_short`

```rust
pub fn permits_short(&self) -> bool {
    // Veto 1
    if self.trend_direction == Some(TrendDirection::Bullish)
        && self.rsi_regime == Some(RegimeState::Bullish)
    {
        warn!("ConfluenceGate: short vetoed — TrendState + RSI both Bullish");
        return false;
    }

    // Veto 2
    if self.ichimoku_cross == Some(IchimokuCrossState::KijunAboveSpanB)
        && self.gaussian_3d == Some(GaussianRegime3D::BullIntact)
    {
        warn!("ConfluenceGate: short vetoed — Ichimoku + GC3D both Bullish");
        return false;
    }

    // Strength veto
    if let Some(events) = &self.rsi_div_4h {
        let has_bullish = events.iter().any(|e| {
            matches!(e, RsiDivEvent::RegularBullish { .. } | RsiDivEvent::HiddenBullish { .. })
        });
        if has_bullish {
            warn!("ConfluenceGate: short suppressed — bullish RSI div on 4H");
            return false;
        }
    }

    true
}
```

#### `size_modifier_long` / `size_modifier_short`

Counts confirming Trend pillars — present and aligned, not just absent.

```rust
pub fn size_modifier_long(&self) -> f64 {
    let mut n = 0u8;
    if self.trend_direction == Some(TrendDirection::Bullish)               { n += 1; }
    if self.rsi_regime      == Some(RegimeState::Bullish)                  { n += 1; }
    if self.ichimoku_cross  == Some(IchimokuCrossState::KijunAboveSpanB)   { n += 1; }
    if self.gaussian_3d     == Some(GaussianRegime3D::BullIntact)          { n += 1; }
    match n { 3 | 4 => 1.0, 2 => 0.75, 1 => 0.5, _ => 0.25 }
}

pub fn size_modifier_short(&self) -> f64 {
    let mut n = 0u8;
    if self.trend_direction == Some(TrendDirection::Bearish)               { n += 1; }
    if self.rsi_regime      == Some(RegimeState::Bearish)                  { n += 1; }
    if self.ichimoku_cross  == Some(IchimokuCrossState::KijunBelowSpanB)   { n += 1; }
    if self.gaussian_3d     == Some(GaussianRegime3D::BearIntact)          { n += 1; }
    match n { 3 | 4 => 1.0, 2 => 0.75, 1 => 0.5, _ => 0.25 }
}
```

---

### Step 2 — Declare module in `src/bot/mod.rs`

```rust
pub mod confluence;
use confluence::ConfluenceGate;
```

No wiring into `run_cycle` yet — that is Task 4.

---

### Files changed

| File | Change |
|------|--------|
| `src/helper/mod.rs` | Add `TRADING_BOT_RSI_DIV_4H`, `TRADING_BOT_RSI_DIV_1D` |
| `src/trackers/rsi_divergence_indicator/mod.rs` | Add `RsiDivSnapshot`, `rsi_div_loop`, `rsi_div_main` |
| `src/tasks/mod.rs` | Spawn 4H and 1D RSI div loops |
| `src/bot/confluence.rs` | Create — full gate struct and all methods |
| `src/bot/mod.rs` | `pub mod confluence; use confluence::ConfluenceGate;` |

---

### Veto logic summary

| Condition | Long blocked | Short blocked |
|-----------|-------------|---------------|
| `TrendState=Bearish` AND `RSI=Bearish` | Yes | — |
| `IchimokuCross=BelowSpanB` AND `GC3D=BearIntact` | Yes | — |
| Recent `RegularBearish` or `HiddenBearish` on 4H | Yes | — |
| `TrendState=Bullish` AND `RSI=Bullish` | — | Yes |
| `IchimokuCross=AboveSpanB` AND `GC3D=BullIntact` | — | Yes |
| Recent `RegularBullish` or `HiddenBullish` on 4H | — | Yes |
| Any signal is `None` (key missing) | Never — fail-open | Never — fail-open |

### Size modifier table

| Confirming Trend pillars (of 4) | Modifier |
|---------------------------------|----------|
| 3–4 | 1.0 (full size) |
| 2 | 0.75 |
| 1 | 0.5 |
| 0 (no veto, just unknown) | 0.25 |