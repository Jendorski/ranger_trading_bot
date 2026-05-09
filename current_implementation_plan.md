# 8th May 2026 Implementation plan

---

## Goal

Complete the SST (Strength, Structure, Trend) confluence gate and wire it into the bot's entry path. Three pillars must all pass before any order is placed. This plan covers only what remains to be built.

---

## SST Pillar Map — Current State

| Pillar | Signal | Data available? | Gate wired? |
|--------|--------|-----------------|-------------|
| **Trend** | `TrendState.direction` (SMC BOS) | Yes — `trading_bot:trend_state` | No |
| **Trend** | Weekly RSI regime (42/45) | Yes — `trading_bot:rsi_snapshot:1W` | No |
| **Trend** | Ichimoku Kijun / Span B cross | Values in Redis, cross never detected | No |
| **Trend** | Gaussian Channel 3D | Not computed | No |
| **Strength** | RSI divergence 4H | Yes — `trading_bot:rsi_div:4H` | No |
| **Strength** | RSI divergence 1D | Yes — `trading_bot:rsi_div:1D` | No |
| **Structure** | SMC zones (supply/demand) | Yes — `trading_bot:zones` | Yes (zone hit triggers entry) |

---

## Task 1 — Ichimoku Kijun / Span B Cross Detection

**File:** `src/trackers/ichimoku/mod.rs`  
**Redis key written:** `trading_bot:ichimoku_cross`  
**Constant to add:** `TRADING_BOT_ICHIMOKU_CROSS` in `src/helper/mod.rs`

### What to build

Add `IchimokuCrossState` enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IchimokuCrossState {
    KijunAboveSpanB,   // bullish: price structure above cloud base
    KijunBelowSpanB,   // bearish: price structure below cloud base
}
```

Add `IchimokuCrossSnapshot`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IchimokuCrossSnapshot {
    pub state: IchimokuCrossState,
    pub updated_at: DateTime<Utc>,
}
```

Detection logic — add inside `process_weekly_ichimoku` after `ichimoku_processor` returns:

```
let kijun  = &weekly_ichimoku.base_line;         // already stored, no adjustment
let span_b = &weekly_ichimoku.leading_span_b;    // already forward-displaced 26 bars

// Use last two valid (non-None) pairs to detect a cross
collect last 2 index positions where both kijun[i] and span_b[i] are Some
if pair[n-2]: kijun >= span_b  AND  pair[n-1]: kijun < span_b  → KijunBelowSpanB (bearish cross)
if pair[n-2]: kijun <  span_b  AND  pair[n-1]: kijun >= span_b → KijunAboveSpanB (bullish cross)
else: carry forward previous state (no cross, keep last known)
```

Write `IchimokuCrossSnapshot` to `trading_bot:ichimoku_cross` after the existing `LAST_25_WEEKLY_ICHIMOKU_SPANS` write.

**Ichimoku runs weekly** (`ichimoku_loop` interval = 604800s). Cross state will be stale-but-valid between weekly refreshes — that is acceptable for a weekly macro gate.

---

## Task 2 — Gaussian Channel 3D Regime Filter

**New file or extension:** add a `GaussianChannel3D` tracker, either as a new module `src/trackers/gaussian_3d/mod.rs` or as an additional instance inside `src/regime/mod.rs`.  
**Redis key written:** `trading_bot:gaussian_regime_3d`  
**Constant to add:** `TRADING_BOT_GAUSSIAN_3D` in `src/helper/mod.rs`

### What to build

The 1W and 2W GC instances already exist in `MacroTracker`. The 3D instance uses the same `GaussianChannel` struct — only the candle feed differs.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GaussianRegime3D {
    BullIntact,     // price above upper channel on 3D
    BearIntact,     // price below lower channel on 3D
    Transitioning,  // price inside channel
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaussianRegime3DSnapshot {
    pub regime: GaussianRegime3D,
    pub upper: f64,
    pub lower: f64,
    pub updated_at: DateTime<Utc>,
}
```

Spawn a new async loop (cadence ~3h = 10800s) that:
1. Fetches 3D candles from Bitget (200 bars sufficient for GC warmup)
2. Feeds bars through `GaussianChannel` instance
3. Compares last close to upper/lower band → emits `GaussianRegime3D` variant
4. Writes `GaussianRegime3DSnapshot` to `trading_bot:gaussian_regime_3d`

Add spawn call in `src/tasks/mod.rs` alongside existing macro tracker spawn.

---

## Task 3 — `ConfluenceGate` struct

**New file:** `src/bot/confluence.rs`  
**Declare module** in `src/bot/mod.rs`: `mod confluence; use confluence::ConfluenceGate;`

### Design

```rust
pub struct ConfluenceGate {
    pub trend_direction:   Option<TrendDirection>,       // from trading_bot:trend_state
    pub rsi_regime:        Option<RegimeState>,          // from trading_bot:rsi_snapshot:1W
    pub ichimoku_cross:    Option<IchimokuCrossState>,   // from trading_bot:ichimoku_cross
    pub gaussian_3d:       Option<GaussianRegime3D>,     // from trading_bot:gaussian_regime_3d
    pub rsi_div_4h:        Option<Vec<RsiDivEvent>>,     // from trading_bot:rsi_div:4H (recent events)
}

impl ConfluenceGate {
    pub async fn read(conn: &mut MultiplexedConnection) -> Self { ... }

    pub fn permits_long(&self) -> bool { ... }
    pub fn permits_short(&self) -> bool { ... }
    pub fn size_modifier(&self) -> f64 { ... }  // 1.0 full, 0.5 reduced
}
```

### `permits_long` logic

```
Trend gate (all three must pass OR missing key = warn + allow):
  trend_direction == Some(Bullish)   OR None (fail-open)
  rsi_regime      == Some(BullIntact) OR None (fail-open)
  ichimoku_cross  == Some(KijunAboveSpanB) OR None (fail-open)
  gaussian_3d     == Some(BullIntact) OR Some(Transitioning) OR None (fail-open)
  HARD VETO: trend_direction == Some(Bearish) AND rsi_regime == Some(BearIntact) → block

Strength gate:
  rsi_div_4h contains no recent RegularBearish or HiddenBearish event (within lookback window)
  OR rsi_div_4h is None (fail-open)
```

`permits_short` is the mirror image.

### `size_modifier` logic

Count how many Trend pillars are actively confirming (not just not-vetoing):
- `trend_direction == Some(Bullish)` for a long
- `rsi_regime == Some(BullIntact)` for a long
- `gaussian_3d == Some(BullIntact)` for a long

2 of 3 confirming → `1.0` (full size)  
1 of 3 confirming → `0.5` (reduced size)  
0 confirming but no hard veto → `0.25` (minimum, or skip)

---

## Task 4 — Wire Gate in `bot/mod.rs`

**Location:** `Position::Flat` branch, between zone-hit detection and order placement.

Current flow:
```
zone hit detected
  → price_action_check
  → order execution
```

Target flow:
```
zone hit detected
  → price_action_check
  → gate = ConfluenceGate::read(&mut self.redis_conn).await
  → if !gate.permits_long() (for long entries): log reason, continue
  → size = base_size * gate.size_modifier()
  → order execution with adjusted size
```

The `if 2 + 2 == 5 { return Ok(()) }` toggle already present can remain as manual override.

---

## Task 5 — `MacroTracker` `levels_ready` Gate

**File:** `src/regime/mod.rs`

`MacroTrackerSnapshot` currently writes to Redis even when fewer than 5 resistance levels have computed values. Add a `levels_ready: u8` field. Gate the Redis write: only publish when `levels_ready == 5`. This prevents the confluence gate from reading a partially-populated snapshot during startup warmup.

Note: `macro_bias` (count of levels price is above) is **not** used as a Trend gate. It is macro context only — do not route it through `permits_long`/`permits_short`.

---

## Priority Table (forward-looking only)

| Priority | Task | File(s) |
|----------|------|---------|
| 1 | Ichimoku Kijun/SpanB cross detection | `trackers/ichimoku/mod.rs`, `helper/mod.rs` |
| 2 | Gaussian Channel 3D loop + snapshot | `trackers/gaussian_3d/mod.rs` or `regime/mod.rs`, `tasks/mod.rs` |
| 3 | `ConfluenceGate` struct — read + permits logic | `bot/confluence.rs` |
| 4 | Wire `ConfluenceGate` into `bot/mod.rs` entry path | `bot/mod.rs` |
| 5 | `size_modifier` — 2-of-3 pillar sizing modulation | `bot/confluence.rs`, `bot/mod.rs` |
| 6 | `MacroTracker` `levels_ready` gate | `regime/mod.rs` |

---

# 23rd April 2026 Implementation plan

---

## Session Context

Reference document studied today:
`/Users/jendorski/Documents/Trading_bots/indicators_reasoning_analysis/signal_logic_generator.md`

The SST (Strength, Structure, Trend) framework from that document drives the decisions below.
All RSI work maps to either the Strength pillar or the macro Trend gate.

---

## RSI — Role in the SST Framework

### Weekly RSI 42–45 (macro regime gate)

- Not overbought/oversold. Treated as a **structural level on the RSI chart itself**.
- Binary: `weekly_rsi > 44` → bull regime possible. `weekly_rsi < 43` → bear market intact.
- Acts as a **sizing modifier**, not a veto:
  - `below_threshold + BearMarketIntact` → +1 to short position sizing
  - `above_threshold + BullMarketIntact` → +1 to long position sizing
  - Conflicting signal → half position size
- Redis key: `trading::rsi_snapshot:1W`

### RSI Trendline Break on 4H (Strength pillar — Archetype 2)

- **Leading signal**: RSI breaks its own trendline before price confirms.
- Critical distinction from signal_logic_generator.md: **decreasing selling pressure ≠ buying strength**.
  - RSI rising from oversold = weak positive signal
  - RSI breaking above trendline + volume expansion = active buying strength
- Detection algorithm:
  1. Compute RSI(14) on 4H bars
  2. Apply pivot detection (lb_left=5, lb_right=5) to RSI value series
  3. Connect last 2 RSI pivot highs → bearish trendline; last 2 pivot lows → bullish trendline
  4. Project trendline to current bar
  5. Cross triggers `RsiMomentumBreak { direction: Bullish | Bearish }`
- Effect: `Bearish` → suppress new longs, prepare short at next KEY_LEVEL below. `Bullish` → suppress new shorts.
- Redis key: `trading::rsi_snapshot:4H`

### RSI Divergence (stronger version of trendline break)

- Regular Bullish (price LL, RSI HL): do NOT enter new shorts. Wait for trendline to actually break.
- Hidden Bearish (price LH, RSI HH): momentum already breaking before price confirms.
- All four types: RegularBullish, HiddenBullish, RegularBearish, HiddenBearish.

---

## Combining `RsiDivEngine` + `RsiRegimeTracker` → Unified `RsiEngine`

### The Redundancy

Both modules implement Wilder's RSI identically. `RsiRegimeTracker` line 200 even notes:
`// Wilder's RSI — identical to RsiDivEngine::update_rsi`

Fields duplicated: `prev_close`, `avg_gain`, `avg_loss`, `rsi_ready`, `init_gains`, `init_losses`, `update_rsi()`, `rsi_value()`.

### The Key Insight

`RsiDivEngine` already maintains `pivot_lows: VecDeque<PivotRecord>` and `pivot_highs: VecDeque<PivotRecord>` (ring buffer, depth=3). These are **exactly** what RSI trendline break detection requires. No new storage needed — trendline projection is an additional check on already-maintained state.

### Three-Layer Architecture

**Layer 1 — `RsiCore` (private shared struct)**
```rust
struct RsiCore {
    len: usize,
    prev_close: Option<f64>,
    avg_gain: f64,
    avg_loss: f64,
    rsi_ready: bool,
    init_gains: Vec<f64>,
    init_losses: Vec<f64>,
}
```
Eliminates RSI duplication at the source.

**Layer 2 — Unified `RsiEngine` with combined event type**
```rust
pub enum RsiEvent {
    Divergence(RsiDivEvent),
    TrendlineBreak {
        direction: Direction,
        rsi_value: f64,
        trendline_value: f64,
        time: DateTime<Utc>,
        bar_index: usize,
    },
    RegimeChange {
        prev: RegimeState,
        next: RegimeState,
        rsi_at_cross: f64,
        threshold: f64,
        time: DateTime<Utc>,
    },
}
```
`process_bar` returns `Vec<RsiEvent>`. One pass per bar, all three signal classes.

**Layer 3 — Async infrastructure stays separate**
`rsi_regime_loop` (Redis + Bitget + seed file) remains as operational code. It instantiates `RsiEngine` instead of `RsiRegimeTracker`. I/O concerns do not merge into the computation engine.

### Trendline Break Addition to `process_bar`

Inside the existing `is_pivot_low` / `is_pivot_high` detection blocks, before the divergence comparison loop:

```
is_pivot_low detected:
  → if pivot_lows.len() >= 2: project bullish trendline from last 2 pivot_lows
  → if rsi_current > projected_value AND rsi_previous <= projected_value:
       emit TrendlineBreak::Bullish
  → [existing divergence comparison loop — unchanged]

is_pivot_high detected:
  → if pivot_highs.len() >= 2: project bearish trendline from last 2 pivot_highs
  → if rsi_current < projected_value AND rsi_previous >= projected_value:
       emit TrendlineBreak::Bearish
  → [existing divergence comparison loop — unchanged]
```

Requires one additional field: `prev_rsi: Option<f64>` on the engine.

---

## Multi-Timeframe RSI Tracker Architecture

The unified `RsiEngine` is timeframe-agnostic. Timeframe semantics come from:
1. Which candles are fed to it
2. `lb_left` / `lb_right` / `range_upper` parameters (bar counts, not seconds)
3. Whether `bear_threshold` / `bull_threshold` are set (weekly only)

### Parameter Table

| Timeframe | `lb_left/right` | `range_upper` | `lb_right` lag | Regime thresholds | Purpose |
|-----------|-----------------|---------------|----------------|-------------------|---------|
| `1W` | 2 / 2 | 52 | 2 weeks | 43.0 / 44.0 | Macro gate |
| `3D` | 3 / 3 | 40 | 9 days | None | Structural confirmation |
| `1D` | 3 / 3 | 50 | 3 days | None | Trend confirmation |
| `4H` | 5 / 5 | 60 | 20 hours | None | Entry strength gate (primary) |
| `1H` | 5 / 5 | 60 | 5 hours | None | Entry refinement |

**Weekly `lb_right` note:** current `default_params()` uses `lb_right=5` which on weekly = 5-week lag. Must use `lb_right=2` or `3` for regime detection to be timely.

**Sub-1H:** RSI pivots on 15m and lower are too noisy for trendline use. Disabled or informational only.

### Multi-Instance Setup

```rust
let weekly_engine = RsiEngine::new(14, 2, 2, 10, 52).with_regime_thresholds(43.0, 44.0);
let daily_engine  = RsiEngine::new(14, 3, 3, 5, 50);
let h4_engine     = RsiEngine::new(14, 5, 5, 5, 60);
let h1_engine     = RsiEngine::new(14, 5, 5, 5, 60);

tokio::spawn(rsi_engine_loop(conn.clone(), "1W",  weekly_engine, 14400));
tokio::spawn(rsi_engine_loop(conn.clone(), "1D",  daily_engine,  3600));
tokio::spawn(rsi_engine_loop(conn.clone(), "4H",  h4_engine,     900));
```

### Redis Key Namespacing

```
trading::rsi_snapshot:1W   → RegimeState, last regime event
trading::rsi_snapshot:1D   → last trendline break direction
trading::rsi_snapshot:4H   → last trendline break direction (primary entry gate)
trading::rsi_snapshot:1H   → entry refinement
```

### Bot Main Loop — How Each TF is Consumed

```
TREND GATE:     trading::rsi_snapshot:1W  → RegimeState (macro sizing modifier)
CONFIRMATION:   trading::rsi_snapshot:1D  → TrendlineBreak direction matches entry?
STRENGTH GATE:  trading::rsi_snapshot:4H  → TrendlineBreak not opposing entry (veto)
REFINEMENT:     trading::rsi_snapshot:1H  → optional entry timing tightening
```

The 4H trendline break is the **entry veto**: `Bearish → suppress new longs`.
The weekly regime is the **sizing weight**: `below 43 + BearMarketIntact → +1 to short size`.

---

## Implementation Priority Order (from signal_logic_generator.md Part 4, updated)

| Priority | Task | Status |
|----------|------|--------|
| 1 | Persist `TrendState` from SMC BOS events to Redis | Not started |
| 2 | Wire Gaussian Channel 3D + Weekly as regime filters | Not started |
| 3 | Wire Ichimoku Kijun/SpanB cross detection (values already in Redis) | Not started |
| 4 | Build unified `RsiEngine` (merge `RsiDivEngine` + `RsiRegimeTracker`) | Designed today |
| 4a | Extract `RsiCore` to eliminate Wilder duplication | Designed today |
| 4b | Add trendline break detection using existing pivot ring buffers | Designed today |
| 4c | Add `RegimeChange` emission via threshold check inline | Designed today |
| 4d | Multi-timeframe instances: 1W, 1D, 4H, 1H | Designed today |
| 5 | Weekly RSI threshold state (absorbed into unified engine) | Designed today |
| 6 | Structural target derivation (replace arithmetic TP) | Not started |
| 7 | LLM signal rule extractor (evolve SentimentClient) | Not started |

---

## Files Relevant to This Work

- [src/trackers/rsi_divergence_indicator/mod.rs](src/trackers/rsi_divergence_indicator/mod.rs) — `RsiDivEngine`, pivot detection, 4 divergence types
- [src/trackers/rsi_regime_tracker/mod.rs](src/trackers/rsi_regime_tracker/mod.rs) — `RsiRegimeTracker`, weekly 43/44 threshold, `rsi_regime_loop`
- [src/trackers/mod.rs](src/trackers/mod.rs) — tracker module registry (both currently declared)
- Reference: `/Users/jendorski/Documents/Trading_bots/indicators_reasoning_analysis/signal_logic_generator.md`