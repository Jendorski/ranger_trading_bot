# Smart Money Concepts (SMC) Engine — Review (Third Pass)

---

## What Changed Since the Last Review

Two fixes were applied:

1. The `PendingSweepHigh` struct was added, snaphotting the active pivot low at sweep detection time.
2. The Bearish BOS sequence check now uses a **price comparison** instead of an index comparison:

```rust
if pending.reference_pivot_low.price >= p_low.price {
```

This resolves the "always true" index bug identified previously. A `StrongHigh` now only fires when the Bearish BOS breaks a pivot low whose price is at or below the reference level captured at sweep time. The fix is correct.

---

## New Finding — StrongLow Has the Same Problem That Was Just Fixed on StrongHigh

The fix was applied asymmetrically. The Bearish BOS / StrongHigh path now uses a `PendingSweepHigh` struct with a reference snapshot. The Bullish BOS / StrongLow path still uses the original raw index check with a plain `Option<Pivot>`:

```rust
// StrongHigh path — FIXED
if pending.reference_pivot_low.price >= p_low.price { ... }

// StrongLow path — NOT fixed
if sweep_low.index > p_high.index { ... }
```

The same two-sided sweep failure applies here. Consider this sequence:

```
Index 5:   Pivot High H1 confirmed     → last_pivot_high = {index:5, price:110}
Index 8:   Sweep Low confirmed         → pending_sweep_low = {index:8, price:90}
Index 12:  Sweep High confirmed        → last_pivot_high = {index:12, price:120}  ← updated
Index 15:  Bullish BOS (close > 120)
           p_high = {index:12, price:120}
           Check: sweep_low.index (8) > p_high.index (12) → 8 > 12 → FALSE
           → StrongLow DISCARDED
```

The market swept a prior low (stop hunt on shorts), then swept a prior high (stop hunt on longs), then broke out higher. That is a valid, textbook long SMC setup. The engine discards it because `last_pivot_high` was updated to the sweep high before the BOS check ran.

**The fix is the same pattern as StrongHigh.** Create a `PendingSweepLow` struct that snapshots the reference pivot high at sweep detection time, then compare by price in the BOS check:

```rust
#[derive(Debug, Clone)]
struct PendingSweepLow {
    sweep: Pivot,
    reference_pivot_high: Pivot,
}
```

When a sweep low is detected:
```rust
if let Some(ref_high) = &self.last_pivot_high {
    self.pending_sweep_low = Some(PendingSweepLow {
        sweep: p.clone(),
        reference_pivot_high: ref_high.clone(),
    });
}
```

In the Bullish BOS check:
```rust
if let Some(pending) = self.pending_sweep_low.take() {
    if pending.reference_pivot_high.price <= p_high.price {
        // BOS broke the same or a higher pivot high than reference → valid
        events.push(SMCEvent::StrongLow { ... });
    }
}
```

This mirrors the StrongHigh fix exactly and closes the gap on the long side.

---

## Remaining Open Issues

---

### Issue — Pending Sweeps Never Expire

Neither `pending_sweep_low` nor `pending_sweep_high` has a time limit. A sweep detected early in the candle batch can activate much later on any BOS that passes the price check.

The engine is re-created fresh every loop tick, so staleness is bounded by the batch size. On 4H charts with 1000 candles that is approximately 167 days. A sweep high from five months ago producing a live short zone is not a useful signal.

The price check (`reference_pivot_low.price >= p_low.price`) provides some structural protection — if price has moved far from the original sweep context, the check may still pass if price is simply lower. This is not a reliable expiry mechanism.

**Recommendation:** Add a configurable max-age in bars to both pending sweeps. Evaluate at BOS time:

```rust
const SWEEP_MAX_AGE_BARS: usize = 50;

let age = idx.saturating_sub(pending.sweep.index);
if age <= SWEEP_MAX_AGE_BARS && pending.reference_pivot_low.price >= p_low.price {
    events.push(SMCEvent::StrongHigh { ... });
}
```

Expose `SWEEP_MAX_AGE_BARS` through `Config` so it can be tuned per timeframe (50 bars on 15m is ~12 hours; on 4H is 8 days).

---

### Issue — Float Equality Used for BOS Deduplication

```rust
self.last_bullish_bos_level.unwrap() != p_high.price
self.last_bearish_bos_level.unwrap() != p_low.price
```

Both sides use `!=` on `f64`. Since prices are stored and retrieved without arithmetic, exact equality holds today. But this is fragile — any future change to the price pipeline (unit conversion, normalisation, rounding) would silently break the deduplication and allow the same BOS to fire repeatedly.

**Recommendation:** Store BOS levels as integers. BTC prices in whole dollars fit in `u64` with no precision loss. At minimum, use an epsilon comparison:

```rust
(self.last_bearish_bos_level.unwrap() - p_low.price).abs() < 0.01
```

---

### Issue — `SweepHigh` Event Can Be Emitted Without a Corresponding `pending_sweep_high`

When a sweep high is detected but `last_pivot_low` is `None` (no pivot low has been confirmed yet), the `SweepHigh` event is still emitted, but `pending_sweep_high` is not set:

```rust
if let Some(ref_low) = &self.last_pivot_low {
    self.pending_sweep_high = Some(PendingSweepHigh { ... });
}
events.push(SMCEvent::SweepHigh { ... });  // emitted regardless
```

Any consumer of the event stream will see a `SweepHigh` that can never produce a `StrongHigh`. This is not a crash risk but is semantically misleading. If the sweep has no structural reference to anchor to, it arguably should not be broadcast at all.

**Recommendation:** Move the `SweepHigh` event emission inside the `if let Some(ref_low)` block so a sweep event is only emitted when a valid pending state can be established:

```rust
if let Some(ref_low) = &self.last_pivot_low {
    self.pending_sweep_high = Some(PendingSweepHigh { ... });
    events.push(SMCEvent::SweepHigh { ... });
}
```

Apply the same treatment to `SweepLow` once the `PendingSweepLow` struct is introduced.

---

### Issue — No Unit Test for the Bearish BOS / StrongHigh Path

`test_strong_low_detection` tests the bullish side end-to-end. The entire Bearish BOS → StrongHigh path remains untested. The sequence bugs identified across three review passes were not caught by automated tests because no such test exists.

**Recommended bar sequence** (with `pivot_left=2, pivot_right=2`):

```
bar 0:  120/120/120/120   ← baseline
bar 1:  121/121/121/121
bar 2:  130/130/130/130   ← Pivot High 1 candidate
bar 3:  121/121/121/121
bar 4:  120/120/120/120   ← confirms Pivot High 1 at index 2
bar 5:  110/110/110/110   ← Pivot Low candidate
bar 6:  120/120/120/120
bar 7:  121/121/121/121   ← confirms Pivot Low at index 5
bar 8:  140/140/140/140   ← Sweep High (> 130)
bar 9:  120/120/120/120
bar 10: 115/115/115/115   ← confirms Sweep High at index 8
bar 11: 105/105/105/105   ← close < Pivot Low (110) → BearishBOS → StrongHigh
```

Assert that `StrongHigh` appears in the emitted events.

Also add a second test covering the **two-sided sweep** scenario (sweep high followed by sweep low followed by bearish BOS) to confirm that case produces a `StrongHigh` and is not silently discarded.

---

## Status Summary

| Item | Status |
|---|---|
| Bearish BOS price comparison fix | ✓ Correct |
| `PendingSweepHigh` reference snapshot | ✓ Correct |
| Bullish BOS / StrongLow — same fix needed | **Gap — index check still used** |
| `SweepHigh` emitted without pending state | **Open** |
| Pending sweep expiry (max-age) | **Open — both sides** |
| Float equality BOS deduplication | **Open — both sides** |
| Unit test for `StrongHigh` | **Not added** |
| Two-sided sweep test (long side) | **Not added** |

The most urgent item is applying the `PendingSweepLow` pattern to the StrongLow path, mirroring the fix already in place for StrongHigh. Without it, the long side has the same structural gap the short side just had.
