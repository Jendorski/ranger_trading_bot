# 9th May 2026 Implementation Plan

---

## Goal

Replace arithmetic SL placement with structurally-derived levels anchored to the SMC zone that triggered entry. Add dynamic SL tightening during the trade based on SST signals. Two layers: initial placement (at entry) and dynamic tightening (on each price poll while in position).

---

## SL Pillar Map — Current State

| Layer | Signal | Data available? | Wired? |
|-------|--------|-----------------|--------|
| **Initial** | Financial risk cap (`margin × risk_pct`) | Yes | Yes — `stop_loss_price()` |
| **Initial** | SMC zone structural anchor (`zone.high` / `zone.low`) | Yes — `trading_bot:zones` | No |
| **Tighten** | RSI Bullish divergence 4H (selling pressure weakening) | Yes — `trading_bot:rsi_div:4H` | No |
| **Tighten** | VRVP HVN above current price (buyers defending) | Yes — `trading_bot:vrvp:4H` | No |
| **Exit** | 4H BullishBOS while short (structural reversal) | Yes — `trading_bot:trend_state` | No |

---

## Task 1 — `StructuralSlLevel` type + `compute_initial_sl`

**New file:** `src/bot/structural_sl.rs`

### `SlSource` enum

```rust
pub enum SlSource {
    SmcZone { zone_high: f64, zone_low: f64 },
    FinancialFallback,
}
```

### `StructuralSlLevel`

```rust
pub struct StructuralSlLevel {
    pub price: f64,
    pub source: SlSource,
}
```

### `compute_initial_sl`

```rust
pub fn compute_initial_sl(
    entry_price: f64,
    pos: Position,
    triggering_zone: &Zone,
    financial_sl: f64,         // output of existing stop_loss_price()
    buffer_multiplier: f64,    // e.g. 0.5 — fraction of zone width used as buffer
) -> StructuralSlLevel
```

**Logic for `Position::Short`:**

```
zone_width   = triggering_zone.high - triggering_zone.low
buffer       = zone_width × buffer_multiplier
structural   = triggering_zone.high + buffer

// Never risk more than the financial cap
sl_price = max(financial_sl, structural)

if structural <= financial_sl:
    source = SmcZone { zone_high, zone_low }
else:
    source = FinancialFallback   // structural was too wide, financial cap wins
```

**Logic for `Position::Long`:** mirror — `zone.low - buffer`, `min(financial_sl, structural)`.

**Why `max` not just structural?**
The zone anchor can be wider than the financial risk cap (e.g. a wide supply zone). The financial cap is the hard ceiling on loss; structural is the preferred level when it fits inside that cap.

---

## Task 2 — Wire into entry path

**File:** `src/bot/mod.rs`

The triggering zone is already in scope in the entry path (the `zone` variable from the `find()` call). Pass it into `compute_initial_sl` alongside the existing financial SL:

```
// existing
let sl = Helper::stop_loss_price(entry_price, margin, leverage, risk_pct, pos);

// replace with
let financial_sl = Helper::stop_loss_price(entry_price, margin, leverage, risk_pct, pos);
let structural_sl = compute_initial_sl(
    entry_price,
    pos,
    zone,                          // already in scope
    Helper::decimal_to_f64(financial_sl),
    config.sl_buffer_multiplier,   // new config key, default 0.5
);
self.open_pos.sl = Some(Helper::f64_to_decimal(structural_sl.price));
```

**New config key to add in `src/config/mod.rs`:**
`SL_BUFFER_MULTIPLIER` — `f64`, default `0.5`.

**Logging:**
```
INFO  Structural SL (short): zone_high=79650 buffer=92 → sl=79742  [SmcZone]
INFO  Structural SL (short): financial cap hit → sl=80652           [FinancialFallback]
```

---

## Task 3 — `evaluate_sl_tighten` — dynamic tightening

**File:** `src/bot/structural_sl.rs`

Called on each price poll while `pos != Flat`, after the partial profit check.

```rust
pub async fn evaluate_sl_tighten(
    conn: &mut MultiplexedConnection,
    current_price: f64,
    current_sl: f64,
    entry_price: f64,
    pos: Position,
) -> Option<f64>   // Some(new_sl) if tighten warranted, None if no change
```

### Signal 1 — RSI Bullish divergence on 4H (while short)

Read `trading_bot:rsi_div:4H`. If any `RegularBullish` or `HiddenBullish` event is present:

```
// Find nearest VRVP 4H HVN above current_price — tighten SL to just above it
candidate_sl = nearest_hvn_above_price(vrvp_4h, current_price).price_high + small_buffer

// Only ever tighten, never widen
if candidate_sl < current_sl:
    return Some(candidate_sl)
```

### Signal 2 — VRVP 4H HVN directly above price (while short)

Read `trading_bot:vrvp:4H`. If current price is within one bin width below an HVN's `price_low`:

```
// Price is approaching an HVN from below — buyers are defending above
candidate_sl = hvn.price_high + small_buffer

if candidate_sl < current_sl:
    return Some(candidate_sl)
```

### Signal 3 — 4H BullishBOS while short (hard exit)

Read `trading_bot:trend_state`. If `direction == Bullish` and `last_bos_time > entry_time`:

```
// Structural basis for the short is gone — exit immediately
return Some(current_price)   // sentinel: caller treats this as close-now
```

**Caller logic in `bot/mod.rs`:**

```rust
if let Some(new_sl) = evaluate_sl_tighten(&mut conn, price, current_sl, entry, pos).await {
    if new_sl == current_price {
        warn!("StructuralSL: 4H BullishBOS while short — immediate exit");
        // close position
    } else {
        self.open_pos.sl = Some(Helper::f64_to_decimal(new_sl));
        exchange.modify_market_order(&self.open_pos).await?;
        warn!("StructuralSL: tightened to {new_sl:.2}");
    }
}
```

---

## Task 4 — `nearest_hvn_above_price` / `nearest_hvn_below_price` helpers

**File:** `src/bot/structural_sl.rs` (shared with TP plan — move to a common location if both plans are built together)

```rust
pub fn nearest_hvn_above_price(profile: &VrvpProfile, price: f64) -> Option<&VrvpNode>
pub fn nearest_hvn_below_price(profile: &VrvpProfile, price: f64) -> Option<&VrvpNode>
```

Simple filter + sort on `profile.nodes`. Also needed by the TP plan (Task 2 there reads VRVP HVNs from the same profile).

---

## Priority order

| # | Task | File |
|---|------|------|
| 1 | `StructuralSlLevel` + `compute_initial_sl` | `src/bot/structural_sl.rs` (new) |
| 2 | Wire initial SL into entry path + `SL_BUFFER_MULTIPLIER` config | `src/bot/mod.rs`, `src/config/mod.rs` |
| 3 | `evaluate_sl_tighten` — RSI div + VRVP signals | `src/bot/structural_sl.rs` |
| 4 | Hard exit on 4H BullishBOS while short | `src/bot/mod.rs` |
| 5 | `nearest_hvn_above/below_price` helpers (shared with TP plan) | `src/bot/structural_sl.rs` |

---

## What stays unchanged

- `stop_loss_price()` — kept as the financial cap input to `compute_initial_sl`
- `ssl_hit()` — unchanged, still the trigger check on each poll
- SL stepping as TPs are hit (`target.sl`) — unchanged

---

# 9th May 2026 — Structural TP Plan

---

## Goal

Replace the arithmetic TP ladder with levels anchored to real structural price levels: SMC long zones and VRVP HVN nodes below entry (for shorts), above entry (for longs). The fraction ladder and SL-stepping logic stay unchanged — only the target prices change.

---

## TP Pillar Map — Current State

| Source | Data available? | Used for TP? |
|--------|-----------------|--------------|
| Nearest SMC zone distance ÷ 4 (arithmetic) | Yes | Yes — current behaviour |
| All SMC zones between entry and target | Yes — `trading_bot:zones` | No |
| VRVP 4H HVN nodes | Yes — `trading_bot:vrvp:4H` | No |
| VRVP 1D HVN nodes | Yes — `trading_bot:vrvp:1D` | No |

---

## Current flow (to be replaced)

```
store_partial_profit_targets(entry, pos)
  → determine_profit_difference()      ← finds nearest zone only
  → total_distance ÷ 4 = step
  → [entry-step, entry-2step, entry-3step, entry-4step]   ← arithmetic
```

---

## Target flow

```
store_partial_profit_targets(entry, pos)
  → collect_structural_tp_levels(conn, entry, pos)         ← NEW
      reads: trading_bot:zones  (SMC long/short zones)
      reads: trading_bot:vrvp:4H  (nearby HVNs)
      reads: trading_bot:vrvp:1D  (farther HVNs)
      → merges, deduplicates, sorts nearest-first
      → returns up to 4 StructuralTpLevel
  → < 4 found: pad remainder with arithmetic fallback
  → 0 found:   full arithmetic fallback (existing behaviour preserved)
  → build_profit_targets_structural(levels, fractions, entry, margin, leverage)
```

---

## Task 1 — `StructuralTpLevel` type

**New file:** `src/bot/structural_tp.rs`

```rust
pub enum TpSource {
    SmcZone,
    VrvpHvn { timeframe: String },
}

pub struct StructuralTpLevel {
    pub price: f64,
    pub source: TpSource,
    pub distance_from_entry: f64,
}
```

---

## Task 2 — `collect_structural_tp_levels`

**File:** `src/bot/structural_tp.rs`

```rust
pub async fn collect_structural_tp_levels(
    conn: &mut MultiplexedConnection,
    entry_price: f64,
    pos: Position,
    min_distance: f64,    // reuse smc_min_distance to deduplicate overlapping levels
    max_levels: usize,    // 4
) -> Vec<StructuralTpLevel>
```

**Logic for `Position::Short` (mirror for Long):**

1. **SMC zones** — read `trading_bot:zones`, take `long_zones` where `zone.high < entry_price`. Use `zone.high` as the TP price (top of the demand zone — where buyers are expected to first push back).

2. **VRVP 4H HVNs** — read `trading_bot:vrvp:4H`, filter `nodes` where `node_type == HighVolumeNode` and `bin.price_mid < entry_price`. Use `bin.price_mid` as the TP price.

3. **VRVP 1D HVNs** — same as above from `trading_bot:vrvp:1D`, for levels that are further from entry.

4. **Merge + deduplicate** — if an SMC zone and a VRVP HVN are within `min_distance` of each other, keep only one (prefer SMC zone). A coincident SMC + HVN means double confirmation; log it.

5. **Sort nearest-first**, take up to `max_levels`.

---

## Task 3 — `build_profit_targets_structural`

**File:** `src/helper/mod.rs` — add alongside existing `build_profit_targets`

Same fraction ladder (`[0.20, 0.30, 0.30, 0.20]`) and same SL-stepping logic. Only `tp_prices` change — sourced from `StructuralTpLevel.price` instead of arithmetic steps.

```rust
pub fn build_profit_targets_structural(
    levels: Vec<StructuralTpLevel>,   // sorted nearest-first, max 4
    entry_price: Decimal,
    margin: Decimal,
    leverage: Decimal,
    pos: Position,
    fallback_step: Decimal,           // ranger_price_difference, for padding
) -> Vec<PartialProfitTarget>
```

**Padding rule:** if only 2 structural levels found, TP3 and TP4 are arithmetic from the last structural level using `fallback_step`.

---

## Task 4 — Wire into `store_partial_profit_targets`

**File:** `src/bot/mod.rs`

```rust
let structural_levels = collect_structural_tp_levels(
    &mut self.redis_conn,
    entry_price,
    pos,
    self.config.smc_min_distance,
    4,
).await;

let ppt = if structural_levels.is_empty() {
    // full arithmetic fallback — existing behaviour unchanged
    Helper::build_profit_targets(
        dec_entry_price, current_margin, dec_leverage,
        dec_ranger_price_difference, pos,
    )
} else {
    Helper::build_profit_targets_structural(
        structural_levels,
        dec_entry_price,
        current_margin,
        dec_leverage,
        pos,
        Decimal::from_f64(self.config.ranger_price_difference).unwrap(),
    )
};
```

---

## Task 5 — Logging per level with source

```
INFO  TP1 @ 79100.00 ← SMC long zone [79050–79150]
INFO  TP2 @ 78800.00 ← VRVP 4H HVN (mid=78800, vol=1204.3)
INFO  TP3 @ 78400.00 ← VRVP 1D HVN (mid=78400) + SMC zone [78350–78450]  (double confirmation)
INFO  TP4 @ 78050.00 ← arithmetic fallback (no structural level found)
```

---

## Priority order

| # | Task | File |
|---|------|------|
| 1 | `StructuralTpLevel` + `collect_structural_tp_levels` | `src/bot/structural_tp.rs` (new) |
| 2 | `build_profit_targets_structural` | `src/helper/mod.rs` |
| 3 | Wire into `store_partial_profit_targets` | `src/bot/mod.rs` |
| 4 | Logging per level with source | `src/bot/mod.rs` |

---

## What stays unchanged

- Fraction ladder: `[0.20, 0.30, 0.30, 0.20]`
- SL-stepping logic as each TP is hit
- `determine_profit_difference` — kept as the arithmetic fallback path
- `build_profit_targets` — kept, called when 0 structural levels found

---

## Shared helpers with SL plan

`nearest_hvn_above_price` / `nearest_hvn_below_price` are needed by both plans. Build them once in `src/bot/structural_sl.rs` and import from `structural_tp.rs`.

---

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