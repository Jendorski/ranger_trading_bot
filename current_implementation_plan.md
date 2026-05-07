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