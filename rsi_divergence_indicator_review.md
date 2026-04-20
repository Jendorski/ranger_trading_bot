# 18th April 2026

## RSI Divergence Indicator — Updated Review

This is a substantially revised version of the file. All three critical issues from the 15th April review have been addressed. The analysis below covers what changed, what is now correct, and what new issues the revision introduces.

---

### What Was Fixed

| Issue (15th April) | Status |
|---|---|
| RSI-pivot design undocumented | Fixed — full module-level doc block added with BTC wick warning and SMC confluence guidance |
| Single-pivot memory, non-adjacent divergences lost | Fixed — `PIVOT_MEMORY = 3` ring buffer replaces single record |
| No RSI zone filter | Fixed — `with_rsi_filter()` builder added with `bull_rsi_max` / `bear_rsi_min` |
| No magnitude/delta fields in events | Fixed — `prev_rsi_value`, `prev_pivot_price`, `rsi_delta`, `price_delta_pct` added to all four variants |
| Triple `VecDeque` synchronization risk | Fixed — consolidated into single `VecDeque<WindowEntry>` |
| Hidden divergences untested | Fixed — `test_hidden_bullish_divergence` and `test_hidden_bearish_divergence` added with analytically verified RSI arithmetic |
| Ring buffer recovery untested | Fixed — `test_non_adjacent_divergence_recovered` confirms A→C fires when B replaces A in the old code |
| Zone filter untested | Fixed — `test_rsi_zone_filter_suppresses_signal` verifies suppression against a known RSI value (≈67.8) |

All 7 tests pass.

---

### Remaining Issues

**1. Two compiler warnings present**

The test run produces two live warnings that should be resolved before integration:

- `PivotRecord.time` is never read. The field is stored but never accessed in any divergence logic or emitted in any event. Either use it — e.g. include `prev_pivot_time: DateTime<Utc>` in event variants so downstream logic knows when the anchor pivot formed — or remove it. Dead fields in internal structs silently accumulate as the codebase grows.

- `default_params()` is never used anywhere in the binary. This is fine for a standalone library module, but the warning signals the engine is not yet wired into the bot's tracker loop or main execution path.

**In plain terms:** The compiler is flagging two things that exist in the code but are never actually used. The `time` field inside `PivotRecord` is stored on every pivot but never read back. It just sits there ignored — either use it or delete it, because dead fields are the kind of thing future developers assume must matter. The `default_params()` warning tells you the engine has not been plugged into the live bot yet. It is built and tested in isolation, but nothing in the trading loop is feeding bars to it or listening for its signals.

---

**2. Multiple events can fire from one pivot confirmation**

With the ring buffer holding up to 3 pivots, a single `process_bar()` call can now return more than one event of the same type. If pivots A, B, and C are all stored, and a new pivot D arrives satisfying divergence conditions against all three, three `RegularBullish` events emit in a single call — all pointing to the same confirmation bar. Downstream signal logic must handle this. Currently nothing in the module documentation warns about it. Without a dedup or priority rule, a naive consumer could size into a position three times on one bar.

The practical recommendation: document this explicitly, and consider emitting only the strongest divergence per bar (highest `rsi_delta`) rather than all matching pivots. On BTC, acting on the most significant divergence is almost always preferable to stacking identical signal types.

**In plain terms:** In the old code, the engine remembered one previous pivot so one new pivot could produce at most one signal. Now it remembers up to three, which means one new pivot can match against all three and fire three signals at once — all from the same bar, all the same type. If the code consuming these signals isn't expecting this and simply acts on every event it receives, it could open three separate trade entries on the same bar thinking it got three independent signals. On BTC with leverage, that is potentially 3x the intended position size from a single candle. The engine should only emit the strongest signal per bar and discard the weaker ones.

---

**3. Zone filter applies uniformly to hidden and regular divergences**

The `bull_rsi_max` filter suppresses both `RegularBullish` and `HiddenBullish` when `cand_rsi >= bull_rsi_max`. This is correct for regular divergences — a regular bullish at RSI 65 is noise. But hidden bullish divergences are trend continuation signals in an established uptrend, and they can validly fire at any RSI level. Filtering them by the same zone threshold can suppress legitimate continuation setups. The two signal types warrant separate filter thresholds.

**In plain terms:** The zone filter was added to stop low-quality signals from firing — for example, stopping a bearish signal from firing when RSI is at 35 because the market is already weak and there is nothing to reverse. That logic makes sense for regular divergences, which are reversal signals. But hidden divergences are different — they are trend continuation signals. A hidden bullish means "the uptrend is still intact, price just pulled back." That signal is valid whether RSI is at 30, 50, or 65, because it is not calling for a reversal. Right now, both regular and hidden divergences get filtered by the same threshold, so a perfectly valid hidden bullish continuation signal in a healthy BTC uptrend can get suppressed simply because RSI is above the cutoff. The two types need separate filter settings.

---

**4. `init_gains` / `init_losses` still not cleared after warmup**

The 112-byte allocation from the previous review is still live after `rsi_ready` transitions to `true`. Negligible in isolation. In a live bot running multiple engine instances (e.g. one per timeframe), it accumulates. A single `self.init_gains = Vec::new()` and `self.init_losses = Vec::new()` at the `rsi_ready = true` transition resolves it.

**In plain terms:** When the engine first starts up, it collects 14 price changes to calculate the opening RSI value. After that, those collections are never needed again — the engine switches to a rolling calculation. But the memory holding those 14 numbers is never released. On its own it is trivially small. The issue is scale: if the bot runs one engine per timeframe — 1m, 5m, 15m, and 1h all simultaneously — that is four dead allocations sitting for the lifetime of the process. Two lines of code at the transition point would clean it up entirely.

---

**5. `range_lower` exact-boundary behaviour untested**

There is no test that places two pivots exactly `range_lower` bars apart. The check uses `>=` ([line 368](src/trackers/rsi_divergence_indicator/mod.rs#L368), [line 425](src/trackers/rsi_divergence_indicator/mod.rs#L425)), so the boundary is inclusive — a pivot exactly at `range_lower` distance should fire. This is not verified. An off-by-one in a future refactor of that condition would go undetected.

**In plain terms:** The engine has a rule that two pivots must be at least `range_lower` bars apart before a divergence can fire. The check is written to include the exact boundary — so two pivots exactly `range_lower` bars apart should be allowed. But there is no test that actually puts two pivots at that exact distance to confirm it. All the tests use distances comfortably inside the range. Boundary conditions — the exact edge of a rule — are the most common place for off-by-one mistakes. If someone later changes that check by accident, no test would catch it.

---

### Architecture Assessment (Updated)

The revised engine is now production-quality for use as a signal source. The module documentation is the strongest part of the update — explicitly calling out the RSI-pivot design decision, the BTC wick implication, the SMC confluence requirement, and the per-timeframe `range_upper` table removes ambiguity for anyone consuming `RsiDivEvent`s downstream.

The ring buffer and magnitude fields together mean downstream signal logic can now filter by both recency (how many pivots ago) and strength (how far RSI diverged), which is the minimum needed to avoid trading weak or overlapping signals.

The one architectural decision still outstanding is the multiple-events-per-bar behaviour introduced by the ring buffer. This needs to be resolved at the consumption layer before orders can be placed safely off these events.

---

# 16th April 2026

## Key Findings

Two critical trading issues:

1. **Pivot detection is RSI-first, not price-first** ([lines 228-229](src/trackers/rsi_divergence_indicator/mod.rs#L228-L229)). Classical divergence finds price swing pivots then reads RSI there. This engine finds RSI extrema and reads price there. On volatile BTC sessions with wick-heavy candles, this means you can get "divergence" at a wick that isn't a real structural swing — the recorded `pivot_price` is the bar's `low`/`high` but the bar may have closed mid-range. Fine if intentional (matches Pine Script), but needs explicit documentation for downstream consumers.

2. **Single-pivot memory means non-adjacent divergences are invisible** ([lines 255-261](src/trackers/rsi_divergence_indicator/mod.rs#L255-L261)). If Pivot B is out of `range_upper`, it replaces Pivot A — so a valid A→C divergence (where A-to-C is in-range) is permanently lost. On BTC 15m during multi-hour trends, this will skip the dominant structural divergences regularly.

Signal quality gaps:

- No RSI zone filter (bearish divergence at RSI 35 shouldn't fire)
- Events carry no magnitude/delta data, so downstream logic can't grade signal strength
- Hidden divergences have zero test coverage — if someone flips a `>` to `<` in lines 244-251 or 283-289, nothing catches it

Solid work: The Wilder smoothing is correctly implemented (two-phase SMA seed → EMA), the no-repaint confirmation pattern is correct, and the regular divergence test for bearish has analytically verified RSI arithmetic which shows serious rigor.

---

## Key Findings — Plain English Explanations

---

### 1. No RSI Zone Filter

RSI is a 0–100 scale. Above 70 means the market has run up hard (overbought). Below 30 means it has sold off hard (oversold). The middle ground (30–70) is neutral.

A bearish divergence means: "price made a higher high but RSI made a lower high — momentum is fading, a drop is coming." That signal only makes sense when RSI is elevated, i.e. the market is actually overbought and momentum is genuinely rolling over from a high place.

If RSI is sitting at 35 (firmly in oversold/neutral territory) and the engine fires a bearish divergence, it is technically correct math but a bad signal in practice. You would be calling for a drop when the market is already beaten down. There is no overbought exhaustion to reverse. You would be shorting into weakness instead of shorting into strength.

The same logic applies in reverse for bullish divergences. A bullish divergence should only fire when RSI is low — say, below 40 or 50. If RSI is at 65 and the engine detects a "bullish divergence," you would be calling for a bounce when the market is already running hot. The signal is technically valid by the math but has no practical edge.

Without a zone filter, the engine treats all divergences equally regardless of where RSI actually sits. A bearish signal at RSI 35 and a bearish signal at RSI 72 get the same weight — but one of those is a high-probability setup and the other is noise.

---

### 2. Events Carry No Magnitude/Delta Data

Every divergence event the engine fires looks the same from the outside. It tells you what type it is, what the RSI value was at the pivot, what the price was, and when it happened. But it does not tell you how strong the divergence was.

Imagine two regular bullish divergences on BTC:

**Divergence A:**
- Price dropped from $95,000 to $94,800 (down 0.2%)
- RSI moved from 38 to 39 (barely moved)

**Divergence B:**
- Price dropped from $95,000 to $88,000 (down 7.4%)
- RSI moved from 38 to 52 (jumped significantly)

Both fire as `RegularBullish`. The engine treats them identically. But Divergence B is far more meaningful — price sold off hard while momentum actually improved strongly. That is a genuine sign that selling pressure is exhausting. Divergence A is barely worth noticing.

Delta just means the difference between the two pivot readings: how much lower (or higher) did price go between the two pivots, and how much did RSI move in the opposite direction. Without those numbers attached to the event, any code consuming these signals has to treat a tiny whisper divergence the same as a strong, clean one. You cannot filter for "only fire on divergences where RSI improved by at least 5 points" — the data simply is not there.

The engine finds divergences correctly, but ships them all in the same plain wrapper with no label saying how convincing the divergence actually is. Grading signal strength requires knowing both pivots, not just the current one.

---

### 3. Single-Pivot Memory Means Non-Adjacent Divergences Are Invisible

The engine only remembers one pivot at a time per direction. Every time a new pivot forms, it completely overwrites the previous one.

Walk through this scenario on BTC 15m, with `range_upper = 60` bars (15 hours on a 15m chart):

- **Pivot A** forms at bar 10 — BTC swings low at $90,000. The engine stores this.
- **Pivot B** forms at bar 85 — BTC swings low at $87,000. Distance is 75 bars, outside the 60-bar limit. No divergence fires. But Pivot A is permanently replaced by Pivot B in memory.
- **Pivot C** forms at bar 95 — BTC swings low at $86,000. Distance from B is 10 bars, within range. The engine compares B vs C. Nothing interesting.

The problem: A vs C is 85 bars apart — outside the limit on its own. But A was a major structural low at $90,000 and C is at $86,000 with RSI recovering. That is the real divergence — the one a trader would actually draw on the chart. The engine never sees it because A was thrown away when B showed up.

This hurts specifically on BTC 15m multi-hour trends because BTC trends often develop over many hours with multiple smaller pivots along the way. The dominant structural divergence — between the first major swing low and the final exhaustion low — is frequently separated by more than 60 bars. All the intermediate pivots in between keep bumping the memory forward, and the anchor pivot that matters most gets erased. You end up getting signals on the small wiggles within the trend, and missing the big signal that marks the actual turning point.

The fix: instead of remembering only the last pivot, remember the last 2 or 3. When a new pivot forms, check it against all of them. That way, even if Pivot B was too close or too far, Pivot A is still in the picture for comparison against Pivot C.

---

# 15th April 2026

## RSI Divergence Indicator Review
### Add here below this line

---

## Executive Summary

Solid Pine Script port with correct Wilder smoothing and a clean no-repaint confirmation model. The core math and state machine are sound. However, there are structural design choices — most notably detecting pivots on RSI rather than price — that create real-world edge cases for BTC perp trading. Several signal quality filters present in professional-grade divergence tools are absent. Coverage gaps in the test suite leave some failure modes unverified. Recommendations are ordered by trading impact.

---

## 1. Architecture Overview

| Component | Implementation | Notes |
|---|---|---|
| RSI | Wilder's smoothing (EMA-style, phase-1 SMA seed) | Matches `ta.rsi` Pine Script exactly |
| Pivot detection | Local extremum on **RSI window** (not price) | Key divergence from classic methodology |
| Confirmation | `lb_right` bars after pivot center | Correct non-repainting pattern |
| Divergence types | Regular Bullish/Bearish, Hidden Bullish/Bearish | All four classical types covered |
| Pivot memory | Single record per direction (`last_pivot_low`, `last_pivot_high`) | Rolling replacement — matches Pine Script |
| Range filter | Bar-count distance between RSI pivot centers | `[range_lower, range_upper]` |

---

## 2. Critical Issues

### 2.1 Pivot Detection on RSI, Not Price — Conceptually Inverted

**File:** [src/trackers/rsi_divergence_indicator/mod.rs:228-229](src/trackers/rsi_divergence_indicator/mod.rs#L228-L229)

```rust
let is_pivot_low = (0..ci).all(|i| self.rsi_win[i] > cand_rsi)
    && (ci + 1..win_size).all(|i| self.rsi_win[i] > cand_rsi);
```

**Problem:** The engine finds pivots where **RSI** is a local minimum/maximum, then reads the price at that bar. Classical divergence methodology works the other way: find **price** pivots (swing highs/lows), then read RSI at those price pivots.

**Trading impact on BTC:** In volatile BTC sessions (e.g. a wick-heavy sell-off), RSI may spike to an extreme on a bar where price closes mid-range. That bar becomes an RSI pivot but not a meaningful price pivot. The price recorded (`cand_bar.low`) at that RSI pivot is not the true structural swing low, and the divergence comparison becomes noisy.

**Example scenario:**
- Bar A: Sudden BTC wick to 58,000, RSI drops to 22, candle closes at 60,500
- Bar B: Normal pullback, RSI 31, candle low 59,200
- Bar A is detected as the RSI pivot low, price recorded as 58,000 (the wick low)
- Bar B's "lower low" comparison is against a wick, not a structure pivot

This is consistent with the reference Pine Script (which also uses RSI pivots), but it means the signals are inherently tied to RSI momentum structure, not price structure. That is acceptable only if it is a documented, intentional design decision — and downstream consumers of `RsiDivEvent` must understand they are receiving RSI-momentum divergences, not classic price-structure divergences.

**Recommendation:** Either document this clearly at the module level, or add a second detection mode that uses price pivots with a separate window. For confluence with the existing SMC zone engine, price-pivot-first divergence would pair better.

---

### 2.2 `last_pivot_low`/`last_pivot_high` Always Advances — Skips Non-Adjacent Divergences

**File:** [src/trackers/rsi_divergence_indicator/mod.rs:255-261](src/trackers/rsi_divergence_indicator/mod.rs#L255-L261)

```rust
// Advance the "previous pivot low" record — always, regardless of range check
self.last_pivot_low = Some(PivotRecord { ... });
```

The pivot record is replaced unconditionally on every new confirmed pivot, including when the range distance was outside `[range_lower, range_upper]`. This means:

- Pivot A (bar 10) is stored
- Pivot B (bar 80) — distance 70, exceeds `range_upper=60` → no divergence, B now replaces A
- Pivot C (bar 95) — distance 15 from B, within range → divergence check against B only
- **A vs C** (distance 85) is never checked, even though A→C might be the dominant structural divergence

On BTC 15m charts during a multi-hour trend, this will frequently cause the engine to miss significant divergences between structurally important swing lows separated by more than 60 bars.

**Recommendation:** Consider keeping the most recent pivot that was within range, or storing a small ring-buffer of N recent pivots (2-3 is sufficient) and checking all of them, discarding those outside range. This is a common enhancement in institutional divergence tools.

---

## 3. Signal Quality Gaps

### 3.1 No RSI Zone Filter

Many professional divergence systems restrict signal emission:
- **Bullish divergences** only valid when RSI is below a threshold (e.g., < 40 or < 50)
- **Bearish divergences** only valid when RSI is above a threshold (e.g., > 60 or > 50)

The current implementation fires regardless of absolute RSI level. This will generate regular bullish divergence signals in overbought territory (RSI 65 → 70) and regular bearish signals in oversold territory — both of which are low-probability setups on BTC.

**File to modify:** [src/trackers/rsi_divergence_indicator/mod.rs:74-101](src/trackers/rsi_divergence_indicator/mod.rs#L74-L101)

Add optional `bull_rsi_max: Option<f64>` and `bear_rsi_min: Option<f64>` fields. Default `None` preserves current behavior.

---

### 3.2 No Divergence Magnitude Tracking

`RsiDivEvent` fields carry `rsi_value` and `pivot_price` but no measure of divergence magnitude — how far apart the two RSI readings are, or how significant the price discrepancy is. A divergence where price drops 0.1% but RSI rises 0.3 points should not carry the same weight as one where price drops 3% and RSI rises 8 points.

**Trading impact:** Downstream signal logic cannot grade signal quality without re-deriving the previous pivot values. Consider adding `prev_pivot_price: f64`, `prev_rsi_value: f64`, and optionally `rsi_delta: f64` / `price_delta_pct: f64` to each event variant.

---

### 3.3 Hidden Divergences — Asymmetric Usefulness

Hidden divergences (trend continuation) are included but carry different risk profiles than regular divergences:

- **Hidden Bullish** (price Higher Low, RSI Lower Low): valid continuation signal in an uptrend — works well on BTC during consolidation above support
- **Hidden Bearish** (price Lower High, RSI Higher High): valid in downtrends — but on BTC specifically, hidden bearish signals in a bull market generate frequent false continuation signals

No separation of these by market context exists. The `RsiDivEngine` is context-blind; it has no awareness of higher-timeframe trend direction. For BTC perp trading, hidden divergences without trend filter will produce a high rate of premature continuation signals during trend reversals.

---

## 4. RSI Implementation — Correctness

### 4.1 Wilder Smoothing: Correct

**File:** [src/trackers/rsi_divergence_indicator/mod.rs:143-177](src/trackers/rsi_divergence_indicator/mod.rs#L143-L177)

The two-phase approach (SMA seed for the first `len` gains/losses, then Wilder's EMA) is the exact Pine Script `ta.rsi` implementation. This is correct.

### 4.2 Edge Case: `avg_loss == 0.0`

**File:** [src/trackers/rsi_divergence_indicator/mod.rs:180-185](src/trackers/rsi_divergence_indicator/mod.rs#L180-L185)

```rust
fn rsi_value(&self) -> f64 {
    if self.avg_loss == 0.0 {
        return 100.0;
    }
    100.0 - 100.0 / (1.0 + self.avg_gain / self.avg_loss)
}
```

`avg_loss == 0.0` is handled correctly (returns 100). The inverse case (`avg_gain == 0.0`, `avg_loss > 0.0`) evaluates to `100 - 100/(1+0)` = 0.0 — also correct. No division-by-zero risk exists.

### 4.3 `init_gains`/`init_losses` Vecs Are Never Cleared

**File:** [src/trackers/rsi_divergence_indicator/mod.rs:87-88](src/trackers/rsi_divergence_indicator/mod.rs#L87-L88)

After `rsi_ready` transitions to `true`, the `init_gains` and `init_losses` vecs are no longer accessed but remain allocated in memory. For a long-running bot feeding thousands of bars, this is a minor memory leak — `len` f64 values per vec, so 14 × 8 bytes = 112 bytes total, negligible but worth a `clear()` call or converting to `Option<Vec<f64>>` that drops after seeding.

---

## 5. Rolling Window Implementation

**File:** [src/trackers/rsi_divergence_indicator/mod.rs:200-215](src/trackers/rsi_divergence_indicator/mod.rs#L200-L215)

The triple `VecDeque` (rsi_win, bar_win, idx_win) approach is functionally correct but has a structural coupling risk: all three are always pushed/popped together, but there is no struct enforcing their synchronization. A future refactor that touches one but not the others would silently desync the windows.

**Recommendation:** Wrap the three into a single `struct WindowEntry { rsi: f64, bar: Bar, global_idx: usize }` and maintain a single `VecDeque<WindowEntry>`. This eliminates the synchronization risk and reduces the window management code from 6 push/pop calls to 2.

---

## 6. Test Coverage Gaps

Current tests cover:
- Regular Bullish (happy path)
- Regular Bearish (happy path, with careful RSI arithmetic verification)
- Out-of-range pivot suppression

**Missing test scenarios:**

| Missing Test | Risk if Absent |
|---|---|
| Hidden Bullish divergence | Hidden divergence logic in lines 244-251 is untested — a typo in the `>` vs `<` comparisons would go undetected |
| Hidden Bearish divergence | Same concern for lines 283-289 |
| Boundary: distance exactly `range_lower` | Off-by-one on `>=` vs `>` unverified |
| Boundary: distance exactly `range_upper` | Off-by-one on `<=` vs `<` unverified |
| RSI value == 100.0 (all gains, no losses) | Edge case in `rsi_value()` — does the pivot still detect correctly? |
| No divergence between valid-range pivots with same-direction price+RSI | Verify the engine correctly emits nothing when price and RSI move in the same direction |
| `lb_left = 1`, `lb_right = 1` (minimum window) | Ensures the window math holds at edge parameters |

The `test_regular_bearish_divergence` test is well-engineered — the RSI arithmetic is analytically derived and documented inline. This level of rigor should be applied to the hidden divergence tests as well.

---

## 7. Integration Considerations for BTC Perp Trading

### 7.1 Timeframe Sensitivity of `range_upper`

Default `range_upper = 60` bars. On a 5-minute chart, this is 5 hours. On a 1-hour chart, this is 60 hours (2.5 days). The engine is timeframe-agnostic, so the caller is responsible for passing appropriate parameters. This should be documented at the `default_params()` constructor level — currently it only says "Pine Script defaults" without noting that the defaults assume a specific timeframe.

### 7.2 No Integration with SMC Zone Context

The engine is purely standalone. For production use in the bot, emitted `RsiDivEvent`s will need to be filtered against active SMC zones from the `SmcEngine`. A regular bearish divergence inside a supply zone is a high-probability short setup; the same divergence in open space with no zone context is far weaker. This filtering logic needs to live in the calling layer, not here — but it means raw events from this engine should **never** directly trigger orders without zone confluence.

### 7.3 Confirmation Latency

With `lb_right = 5` (default), every signal is confirmed 5 bars late. On a 1-minute BTC chart, that is 5 minutes of lag — acceptable for swing entries. On a 5-minute chart, it is 25 minutes. Callers must account for this when sizing entries and placing stops relative to the confirmed pivot price (which may be significantly above/below the current market price by confirmation time).

---

## 8. Summary of Recommendations

| Priority | Issue | Action |
|---|---|---|
| High | RSI pivot vs price pivot ambiguity | Document the design choice explicitly; consider adding a `price_pivot_mode` option |
| High | Single pivot memory misses non-adjacent divergences | Store 2-3 recent pivots per direction |
| High | No RSI zone filter | Add optional `bull_rsi_max` / `bear_rsi_min` thresholds |
| Medium | No divergence magnitude in events | Add `prev_pivot_price`, `prev_rsi_value` to event variants |
| Medium | Triple VecDeque synchronization risk | Consolidate into `VecDeque<WindowEntry>` |
| Medium | Missing hidden divergence tests + boundary tests | Add the 7 test cases listed in section 6 |
| Low | `init_gains`/`init_losses` not cleared after warmup | Add `.clear()` or drop after `rsi_ready = true` |
| Low | `default_params()` timeframe assumption undocumented | Add doc comment noting defaults target 5m/15m candles |