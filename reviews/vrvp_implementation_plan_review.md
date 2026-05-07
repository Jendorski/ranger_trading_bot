# 17th April 2026

**Source of feedback:** Bitcoin desk analyst re-review of
`src/trackers/visible_range_volume_profile/mod.rs` — two passes on the same date.
First pass identified gaps against the 15th April plan; second pass verified which fixes
landed and found new hardening issues in the updated code.

---

## Pass 1 — Status Against 15th April Plan

Cross-checking the code state at the start of the day against the 9-step plan:

| Step | Description | Status |
|---|---|---|
| Step 1 | `bin_count` 100 → 200 | **Done** — `VrvpEngine::new(200)` in `vrvp_main()` |
| Step 2 | Percentile-based HVN/LVN (85th/15th) | **Done** — implemented in `compute()` |
| Step 3 | Value area tie-break `>=` → `>` | **Done** — `compute_value_area()` uses strict `>` |
| Step 4 | Remove `hvn_factor` / `lvn_factor` fields | **Done** — `VrvpEngine` only has `bin_count` and `value_area_pct` |
| Step 5 | `node_at(price)` | **Done** — present on `VrvpProfile` |
| Step 6 | `nearest_hvn_in_direction()` | **Done** — present on `VrvpProfile` |
| Step 7 | `nearest_lvn_in_direction()` | **Done** — present on `VrvpProfile` |
| Step 8 | Tests: query API + bounded HVN count | **Done** — 10 tests covering all new cases |
| Step 9 | Spawn `vrvp_loop` in `main.rs` | **Not verified** — `main.rs` not checked |

All engine-level fixes from the 15th are confirmed in. Step 9 (spawning) flagged for verification.

---

## Pass 1 — New Issues Identified

### Issue 1 — Uniform bin count across all timeframes degrades HTF profile quality

**Location:** `vrvp_main()` — `VrvpEngine::new(200)` applied to every timeframe.

**Problem:** 200 bins is appropriate for 15m (narrow range, ~$3–5k over 333 candles → ~$15–25
per bin). On a Weekly chart covering $40–60k+, 200 bins means ~$200–300 per bin — more
resolution than Weekly structure demands. Excess bins fragment a single institutional HVN into
multiple adjacent bins, diluting the signal. Lower bin counts on higher timeframes produce
cleaner, more tradeable node structure.

**Recommended tiering:**

| Timeframe | Candles | Suggested bins | Approx bin width at $85k |
|---|---|---|---|
| 15m | 333 | 150 | ~$20 |
| 4H | 500 | 100 | ~$100 |
| 1D | 365 | 75 | ~$200 |
| 1W | 52 | 60 | ~$300 |

**Fix:** Pass `bin_count` as a parameter to `vrvp_loop` and `vrvp_main`.

**Status after Pass 2:** **Fixed** — `bin_count` is now a parameter on both functions; doc
comment examples show the tiered config above.

---

### Issue 2 — 1D, 3D, and 1W timeframes have no loops configured

**Location:** Doc comment examples on `vrvp_loop` showed only `15m` and `4H`.

**Problem:** The three highest timeframes in the trading plan — 1D, 3D, 1W — were not
configured. Macro structural levels (annual HVNs, halving-cycle POCs) that inform bias on
the 4H and 15m were absent.

**3D is not a native Bitget candle granularity.** It must be constructed by resampling 1D
bars into 3-day buckets before passing to `VrvpEngine::compute()`. 3D cannot be passed as
a raw `timeframe` string to `fetch_bars()`.

**Status after Pass 2:** **Partially fixed** — doc comment examples now include 1D and 1W
with correct tiered bin counts. 3D still requires a dedicated aggregation task and remains
unimplemented.

---

### Issue 3 — No Redis TTL on stored profiles

**Location:** `vrvp_main()` — `conn.set(redis_key, serialized)` with no expiry.

**Problem:** If a `vrvp_loop` task panics or the process dies, the Redis key persists
indefinitely. Downstream signal logic reads a stale profile hours or days old without any
indication of staleness. A $5,000 price move after the last write renders all HVN/LVN
classifications incorrect.

**Status after Pass 2:** **Fixed** — `set_ex` with TTL `interval_secs * 2` is now in place.

---

### Issue 4 — Silent error swallowing in `fetch_bars`

**Location:** `fetch_bars()` — `res.unwrap_or_default()`.

**Problem:** A Bitget API error silently produced an empty `Vec<Bar>`. The caller logged
"no bar data received" with no way to distinguish a genuine empty response from an exchange
failure.

**Status after Pass 2:** **Fixed** — `fetch_bars` now returns `Result<Vec<Bar>, String>`.
The caller distinguishes API errors (logged at `error`) from empty responses (logged at
`info`).

---

### Issue 5 — No POC migration detection

**Problem:** Each loop iteration fully overwrites the Redis key. There is no comparison
against the previous profile to detect when the POC has shifted significantly. A migrating
POC during a trend is a primary VRVP signal — it indicates the market is building new value
rather than mean-reverting, and should influence bias.

**Status after Pass 2:** Still open — flagged as Future, not blocking current delivery.

---

## Pass 2 — New Issues Found in Updated Code

### New Issue A — Compile-breaking `vrvp_loop` signature change not yet reflected in `main.rs`

The `vrvp_loop` signature changed from 4 arguments to 5 (`bin_count` added). If `main.rs`
still holds the old call, **the binary will not compile**. Step 9 must be completed before
the next build. This is the most urgent remaining item.

---

### New Issue B — Panic risk on timestamp parsing in `fetch_bars`

**Location:** `fetch_bars()` line 302:

```rust
time: Utc.timestamp_millis_opt(c.timestamp).unwrap(),
```

`timestamp_millis_opt` returns a `LocalResult` which can be `None` if `c.timestamp` is
out of chrono's valid range. `.unwrap()` panics on `None`, which permanently kills the
`vrvp_loop` task. A malformed API response or exchange-side data corruption would trigger
this. Use `.unwrap_or_else(|| Utc::now())` or skip the bar entirely on `None`.

---

### New Issue C — JSON serialisation `.unwrap()` can panic on NaN/Inf volumes

**Location:** `vrvp_main()` line 361:

```rust
let serialized = serde_json::to_string(&profile).unwrap();
```

JSON does not support NaN or Inf. If a candle arrives with a NaN high or low from a bad
API response, the value propagates through the volume distribution and ends up in a
`VrvpBin.volume` field. `serde_json` will return an error on serialisation, and the
`.unwrap()` will panic and kill the task. Replace with a `match` that logs the error and
skips the write.

---

### New Issue D — HVN boundary asymmetry produces slightly fewer HVNs than expected

**Location:** `compute()` lines 222 and 224:

```rust
if bin.volume > hvn_threshold {        // strict — bins at exactly the 85th pct → Neutral
} else if bin.volume <= lvn_threshold { // inclusive — bins at exactly the 15th pct → LVN
```

The HVN set is `strictly above` the 85th percentile; the LVN set is `at or below` the 15th.
This asymmetry means on a perfectly uniform distribution the HVN count will be slightly
smaller than 15% of bins and the LVN count slightly larger. Not a bug, but worth knowing
when calibrating the +2/+1 scoring weights in `structure_score()` — the HVN set will
consistently be the smaller of the two.

---

### New Issue E — `value_area_volume` slice relies on implicit `val_idx <= vah_idx` invariant

**Location:** `compute()` line 209:

```rust
let value_area_volume: f64 = bins[val_idx..=vah_idx].iter().map(|b| b.volume).sum();
```

This panics if `val_idx > vah_idx`. The invariant holds because `compute_value_area` starts
both bounds at `poc_idx` and only moves them outward, but the invariant is nowhere
asserted. A `debug_assert!(val_idx <= vah_idx)` here would make the assumption explicit and
catch regressions immediately in test runs.

---

## Updated Checklist

| # | Item | Status |
|---|---|---|
| 1 | `bin_count` 100 → 200 | ✅ Done |
| 2 | Percentile-based HVN/LVN (85th/15th) | ✅ Done |
| 3 | Value area tie-break `>=` → `>` | ✅ Done |
| 4 | Remove unused `hvn_factor` / `lvn_factor` | ✅ Done |
| 5 | `node_at(price)` | ✅ Done |
| 6 | `nearest_hvn_in_direction()` | ✅ Done |
| 7 | `nearest_lvn_in_direction()` | ✅ Done |
| 8 | Tests: query API + bounded HVN count | ✅ Done |
| 9 | `vrvp_loop` spawned in `main.rs` (4H, 1D, 1W) | ⬜ **Urgent — blocks compilation** |
| 10 | `bin_count` parameterised per timeframe | ✅ Done |
| 11 | Redis TTL via `set_ex` | ✅ Done |
| 12 | `fetch_bars` error propagation | ✅ Done |
| 13 | 3D bar aggregation task | ⬜ Open |
| A | Guard timestamp `.unwrap()` in `fetch_bars` (New Issue B) | ⬜ Open |
| B | Guard `serde_json` `.unwrap()` in `vrvp_main` (New Issue C) | ⬜ Open |
| C | `debug_assert!(val_idx <= vah_idx)` in `compute()` (New Issue E) | ⬜ Open |
| — | POC migration detection | ⬜ Future |
| — | Wire `node_at()` into `structure_score()` (+2 HVN / +1 LVN) | ⬜ Future |
| — | Wire directional nav into cascade target derivation (Priority 6) | ⬜ Future |

---

# 15th April 2026
## VRVP Implementation Plan

**Source of feedback:** Bitcoin desk analyst review of
`src/trackers/visible_range_volume_profile/mod.rs` against
`signal_logic_generator.md` (SST framework) and `signal_generator_assesement.md`.

---

## Status Snapshot

| Component | Status | Blocks what |
|---|---|---|
| Volume distribution algorithm | Correct | — |
| POC computation | Correct | — |
| Value area expansion | Correct (minor tie-break bias) | — |
| HVN/LVN classification | Present, thresholds too permissive | `structure_score()` accuracy |
| Redis storage (timeframe-qualified) | Correct | — |
| Background loop (`vrvp_loop`) | Defined, **never spawned** | Everything — profile never exists |
| `node_at(price)` | **Missing** | `structure_score()` HVN +2 / LVN +1 |
| `nearest_hvn_in_direction()` | **Missing** | Priority 6: cascade target derivation |
| `nearest_lvn_in_direction()` | **Missing** | Priority 6: cascade target derivation |
| `bin_count` adequacy | Too coarse (100 → need 200+) | Level resolution at BTC prices |
| `candle_count` adequacy | Too short for structure (150 → 500+ on 4H) | Weekly/daily structural scoring |

---

## Part 1 — Critical Blockers (nothing works without these)

### Blocker A — `vrvp_loop` is never spawned

**Location:** `src/main.rs` — the spawner block that starts SMC and Ichimoku loops (lines 51–66).
`vrvp_loop` is never called. Every Redis key `trading_bot:vrvp:*` is permanently empty.

**Effect:** The VRVP engine is correct but produces zero output. Any code that reads VRVP data
from Redis (future `structure_score()`, cascade targets) will always get a cache miss and
fall back to "no data", silently skipping the +2 HVN and +1 LVN contributions to key level scoring.

**Root cause:** No feature flag exists for VRVP (`use_vrvp_indicator` in `Config`), so there is
no conditional spawn hook analogous to `use_smc_indicator`. The simplest fix is to always spawn
it unconditionally alongside SMC — VRVP is always useful data regardless of the entry strategy.

**Fix:**

Add two spawns to `main.rs` after the existing indicator blocks:

```rust
// 4H VRVP — 500 candles covers ~83 days of structure; refresh every 30 min
let vrvp_conn_4h = redis_conn.clone();
tokio::spawn(async move {
    trackers::visible_range_volume_profile::vrvp_loop(
        vrvp_conn_4h, "4H", "500", 1800,
    ).await;
});

// 1D VRVP — 365 candles covers ~1 year of daily structure; refresh every 2 hours
let vrvp_conn_1d = redis_conn.clone();
tokio::spawn(async move {
    trackers::visible_range_volume_profile::vrvp_loop(
        vrvp_conn_1d, "1D", "365", 7200,
    ).await;
});
```

This writes to `trading_bot:vrvp:4H` and `trading_bot:vrvp:1D` independently.

---

## Part 2 — Missing Query API on `VrvpProfile`

These three methods are the interface between the VRVP engine and the rest of the bot.
Without them, other modules cannot ask VRVP questions without inlining bin scanning logic,
which is both fragile and slow.

### Gap A — `node_at(price) -> NodeType`

**Why it matters:** `structure_score()` (signal doc, Part 2, Structure pillar) awards:
- `+2` if `level == VPVR high-volume node (HVN)`
- `+1` if `level == VPVR low-volume node (LVN) nearby`

Right now there is no way to ask "what is the node type at $83,400?" without deserialising
the entire profile and scanning every bin manually at the call site. That belongs inside
`VrvpProfile`, not in the bot loop.

**Spec:**

```rust
impl VrvpProfile {
    /// Returns the NodeType of whichever bin contains `price`.
    /// Returns `NodeType::Neutral` if price is outside the profile range.
    pub fn node_at(&self, price: f64) -> NodeType {
        self.nodes
            .iter()
            .find(|n| price >= n.bin.price_low && price < n.bin.price_high)
            .map(|n| n.node_type.clone())
            .unwrap_or(NodeType::Neutral)
    }
}
```

**Note on the boundary condition:** The upper bound is exclusive (`price < n.bin.price_high`),
which means a price exactly at the range maximum returns `NodeType::Neutral`. This is correct
— a price at the very top of the visible range has no forward VRVP context.

---

### Gap B — `nearest_hvn_in_direction(price, bullish) -> Option<f64>`

**Why it matters:** Signal doc Priority 6 states:

> `target_1 = next_scored_structural_level(DIRECTION, from=KEY_LEVEL)`

For VRVP specifically, the nearest HVN in the trade direction is the first structural magnet /
brake price will encounter. For a long trade at $83,000, the first HVN above (say $86,500)
is `target_1` — price will stall or consolidate there because institutional volume was traded
at that level. Without this function, Priority 6 cannot read VRVP data at all.

**Spec:**

```rust
impl VrvpProfile {
    /// Returns the midpoint of the nearest HVN strictly above `price` (bullish=true)
    /// or strictly below `price` (bullish=false).
    ///
    /// "Nearest" = smallest absolute distance from current price.
    pub fn nearest_hvn_in_direction(&self, price: f64, bullish: bool) -> Option<f64> {
        if bullish {
            self.nodes
                .iter()
                .filter(|n| n.node_type == NodeType::HighVolumeNode && n.bin.price_low > price)
                .min_by(|a, b| a.bin.price_mid.partial_cmp(&b.bin.price_mid).unwrap())
                .map(|n| n.bin.price_mid)
        } else {
            self.nodes
                .iter()
                .filter(|n| n.node_type == NodeType::HighVolumeNode && n.bin.price_high < price)
                .max_by(|a, b| a.bin.price_mid.partial_cmp(&b.bin.price_mid).unwrap())
                .map(|n| n.bin.price_mid)
        }
    }
}
```

**Caller intent:** When the bot enters a long at $83,000 and calls
`profile.nearest_hvn_in_direction(83_000.0, true)`, it gets back the first HVN above — this
becomes the structural `target_1` for the cascade.

---

### Gap C — `nearest_lvn_in_direction(price, bullish) -> Option<f64>`

**Why it matters:** An LVN beyond the nearest HVN is an "air pocket" — price accelerates
through it once the HVN is cleared. This maps to `target_2` in the cascade and directly to
the analyst's language:

> *"A loss of 66,000 will result in a massive amount of volatility toward the lower support."*
> *"Massive low historical volume ranges sit above 72.2K and below 66,000."*

The LVN is what makes a cascade move fast. Without it, `target_2` defaults to arithmetic TPs
which leave structural targets unrealised.

**Spec:**

```rust
impl VrvpProfile {
    /// Returns the midpoint of the nearest LVN strictly above `price` (bullish=true)
    /// or strictly below `price` (bullish=false).
    pub fn nearest_lvn_in_direction(&self, price: f64, bullish: bool) -> Option<f64> {
        if bullish {
            self.nodes
                .iter()
                .filter(|n| n.node_type == NodeType::LowVolumeNode && n.bin.price_low > price)
                .min_by(|a, b| a.bin.price_mid.partial_cmp(&b.bin.price_mid).unwrap())
                .map(|n| n.bin.price_mid)
        } else {
            self.nodes
                .iter()
                .filter(|n| n.node_type == NodeType::LowVolumeNode && n.bin.price_high < price)
                .max_by(|a, b| a.bin.price_mid.partial_cmp(&b.bin.price_mid).unwrap())
                .map(|n| n.bin.price_mid)
        }
    }
}
```

---

## Part 3 — Technical Fixes to the Existing Engine

These are correctness issues in the current computation. The engine produces output, but the
output is less accurate than it needs to be for the signal scoring use case.

### Fix 1 — `bin_count` too coarse for BTC prices

**Location:** `vrvp_main()` line 281: `VrvpEngine::new(100)`

**Problem:** With 100 bins over a ~$25,000 visible range (typical 4H chart at current BTC
prices), each bin is ~$250 wide. The signal doc identifies structural levels with $200–$500
precision. A BOS level at $83,400 may share a bin with an LVN centred at $83,550 — the
`node_at()` call cannot distinguish them.

**Fix:** Change the default from 100 to 200 bins.

```rust
// In vrvp_main():
let engine = VrvpEngine::new(200);
```

At 200 bins over a $25,000 range, bin width is ~$125 — below the $200 minimum structural
precision used in the scoring rubric. Two structurally distinct levels will always fall in
separate bins.

**Consequence:** Profile is 2× larger in memory and Redis. At 200 bins, the serialised
`VrvpProfile` is ~30–40 KB per timeframe. Negligible.

---

### Fix 2 — HVN/LVN thresholds produce too many nodes

**Location:** `VrvpEngine::compute()`, lines 167–187 (the node classification block).

**Problem:** `hvn_threshold = mean_vol * (1 + hvn_factor)` with `hvn_factor = 0.5` classifies
any bin above 1.5× the mean as HVN. On a smooth BTC volume distribution across 200 bins, this
yields 20–40 HVN bins — far too many for structure scoring. When `node_at()` returns HVN for
a wide swath of the profile, the +2 bonus becomes meaningless.

**Fix:** Replace the mean-factor threshold with percentile-based classification.
Classify the **top 15% of bins by volume** as HVN and the **bottom 15%** as LVN.

```rust
// Replace the current classification block in VrvpEngine::compute():

let mut sorted_volumes: Vec<f64> = bins.iter().map(|b| b.volume).collect();
sorted_volumes.sort_by(|a, b| a.partial_cmp(b).unwrap());

let n = sorted_volumes.len();
let hvn_threshold = sorted_volumes[(n as f64 * 0.85) as usize];
let lvn_threshold = sorted_volumes[(n as f64 * 0.15) as usize];

let nodes: Vec<VrvpNode> = bins
    .iter()
    .map(|bin| {
        let node_type = if bin.volume >= hvn_threshold {
            NodeType::HighVolumeNode
        } else if bin.volume <= lvn_threshold {
            NodeType::LowVolumeNode
        } else {
            NodeType::Neutral
        };
        VrvpNode { bin: bin.clone(), node_type }
    })
    .collect();
```

**Why 85th/15th percentile:** Produces at most 30 HVN and 30 LVN bins across 200 total.
This is stable regardless of the distribution shape, which changes as BTC cycles between
high- and low-volatility regimes. Also means the HVN/LVN fields on the engine struct
(`hvn_factor`, `lvn_factor`) become unused — they should be removed to avoid confusion.

---

### Fix 3 — Value area tie-break skews high

**Location:** `VrvpEngine::compute_value_area()`, line 231:

```rust
if next_upper_vol >= next_lower_vol {  // current code
```

**Problem:** When adjacent bins have equal volume, this always expands upward. Over many
expansion steps this is a systematic upward drift — VAH ends up slightly too high, VAL
ends up slightly too high, creating a subtle upward bias in the entire 70% zone.

**Fix:** Change `>=` to `>`. Equal volumes now expand downward (the `else` branch), which
is the more conservative direction.

```rust
if next_upper_vol > next_lower_vol {
```

In practice, exact float equality at non-zero volumes is extremely rare in real candle data,
so this change affects primarily the boundary case of zero-volume bins at the range edges —
which is exactly where you want to be conservative.

---

### Fix 4 — `candle_count` is too short for structural scoring

**Location:** The `candle_count` argument passed to `vrvp_loop` in `main.rs` (not yet added).

**Problem:** The example in the docstring uses `"150"` for 4H. At 4H × 150 candles = 600
hours = ~25 days. The signal doc awards the highest structural weight (+3) to:

> *major swing high/low visible on weekly or daily*

Weekly and daily structural levels form over months. A major swing high/low from 3 months
ago — which the analyst explicitly references — will not appear in a 25-day 4H window at all.

**Required counts by timeframe:**

| Timeframe | Candles | Coverage | Justification |
|---|---|---|---|
| 4H | 500 | ~83 days | Covers ~3 months of 4H structure |
| 1D | 365 | ~1 year | Captures full annual cycle levels |
| 1W | 208 | 4 years | Full BTC halving cycle (future use) |

**Fix:** Use 500 for 4H and 365 for 1D in the spawn calls in `main.rs` (already specified
in Blocker A above).

---

## Part 4 — Implementation Order

The issues are not independent. Order matters:

```
Step 1 — Fix bin_count: 100 → 200
Step 2 — Fix HVN/LVN to percentile-based (85th/15th)   ← Step 1 first for test accuracy
Step 3 — Fix value area tie-break: >= → >               ← independent, do alongside Step 2
Step 4 — Remove now-unused hvn_factor / lvn_factor fields from VrvpEngine
Step 5 — Add VrvpProfile::node_at()
Step 6 — Add VrvpProfile::nearest_hvn_in_direction()     ← after engine fixes so results
Step 7 — Add VrvpProfile::nearest_lvn_in_direction()       are meaningful
Step 8 — Add tests for the three new methods + bounded HVN count
Step 9 — Spawn vrvp_loop in main.rs (4H×500, 1D×365)   ← last; engine must be correct first
```

**Why spawn last:** If the loop runs before the engine fixes, incorrect profiles land in Redis.
Any downstream code that reads those profiles caches incorrect HVN/LVN classifications for
up to 2 hours (1D interval). Fix the engine, verify with tests, then start emitting.

---

## Part 5 — Tests to Add / Update

The existing 5 tests cover the core math and will still pass. These additions cover the
new API and the corrected classification thresholds.

### Tests for the query API

```rust
#[test]
fn test_node_at_returns_hvn_at_dominant_cluster() {
    let bars = vec![
        bar(105.0, 115.0, 500.0), // dominant volume → HVN
        bar(100.0, 110.0, 10.0),
        bar(90.0, 95.0, 2.0),
    ];
    let profile = VrvpEngine::new(50).compute(&bars).unwrap();
    // Centre of the dominant cluster must return HVN
    assert_eq!(profile.node_at(110.0), NodeType::HighVolumeNode);
}

#[test]
fn test_node_at_out_of_range_returns_neutral() {
    let bars = vec![bar(100.0, 200.0, 100.0)];
    let profile = VrvpEngine::new(20).compute(&bars).unwrap();
    assert_eq!(profile.node_at(50.0), NodeType::Neutral);   // below range
    assert_eq!(profile.node_at(300.0), NodeType::Neutral);  // above range
}

#[test]
fn test_nearest_hvn_above_returns_closest() {
    // Two HVN clusters at ~110 and ~130, current price = 100
    let bars = vec![
        bar(108.0, 112.0, 400.0), // HVN ~110
        bar(128.0, 132.0, 400.0), // HVN ~130
        bar(100.0, 200.0, 5.0),   // low-volume noise across full range
    ];
    let profile = VrvpEngine::new(100).compute(&bars).unwrap();
    let hvn = profile.nearest_hvn_in_direction(100.0, true);
    assert!(hvn.is_some());
    assert!(hvn.unwrap() < 130.0, "should return closer HVN (~110), not farther one (~130)");
}

#[test]
fn test_nearest_lvn_below_returns_closest() {
    let bars = vec![
        bar(80.0, 82.0, 0.5),    // LVN ~81
        bar(70.0, 72.0, 0.5),    // LVN ~71
        bar(85.0, 100.0, 500.0), // HVN ~92 (dominant)
    ];
    let profile = VrvpEngine::new(100).compute(&bars).unwrap();
    let lvn = profile.nearest_lvn_in_direction(85.0, false);
    assert!(lvn.is_some());
    assert!(lvn.unwrap() > 70.0, "should return closer LVN (~81), not farther one (~71)");
}

#[test]
fn test_nearest_hvn_returns_none_when_no_hvn_in_direction() {
    // All volume is at the bottom — no HVN above the current price
    let bars = vec![
        bar(80.0, 90.0, 500.0),  // HVN here
        bar(100.0, 200.0, 1.0),  // LVN across the rest
    ];
    let profile = VrvpEngine::new(50).compute(&bars).unwrap();
    // Asking for HVN above $150 should return None (all HVN is below $150)
    let hvn = profile.nearest_hvn_in_direction(150.0, true);
    assert!(hvn.is_none());
}
```

### Test for bounded HVN count under percentile classification

```rust
#[test]
fn test_percentile_hvn_count_bounded() {
    // 40 bars with uniform volume + one outlier
    let mut bars: Vec<Bar> = (0..40)
        .map(|i| bar(i as f64 * 10.0, i as f64 * 10.0 + 9.0, 10.0))
        .collect();
    bars.push(bar(500.0, 509.0, 10_000.0)); // extreme outlier

    let profile = VrvpEngine::new(200).compute(&bars).unwrap();
    let hvn_count = profile
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::HighVolumeNode)
        .count();

    // 15% of 200 bins = max 30 HVN bins
    assert!(hvn_count <= 30, "too many HVN bins: {hvn_count}");
}
```

---

## Part 6 — Integration Sketch (downstream, not in scope of this plan)

Once Steps 1–9 are complete, the call pattern for `structure_score()` and cascade target
derivation is straightforward. This is documented here as reference for when Priority 6
of the signal doc is implemented.

```rust
// structure_score() — awards VRVP contribution
async fn vrvp_score_for(price: f64, conn: &mut MultiplexedConnection) -> i32 {
    let profile_4h = load_vrvp_profile(conn, "4H").await;
    match profile_4h.as_ref().map(|p| p.node_at(price)) {
        Some(NodeType::HighVolumeNode) => 2,
        Some(NodeType::LowVolumeNode)  => 1,
        _                              => 0,
    }
}

// Cascade target derivation — structural targets, not arithmetic
async fn cascade_targets(
    entry: f64,
    bullish: bool,
    conn: &mut MultiplexedConnection,
) -> (Option<f64>, Option<f64>) {
    let Some(profile) = load_vrvp_profile(conn, "4H").await else {
        return (None, None);
    };
    let target_1 = profile.nearest_hvn_in_direction(entry, bullish);
    let target_2 = target_1.and_then(|t1| profile.nearest_lvn_in_direction(t1, bullish));
    (target_1, target_2)
}
```

---

## Summary Checklist

- [ ] **Step 1** — `VrvpEngine::new()` default `bin_count`: 100 → 200
- [ ] **Step 2** — Replace `mean × factor` thresholds with percentile-based (85th/15th)
- [ ] **Step 3** — Value area tie-break: `>=` → `>`
- [ ] **Step 4** — Remove unused `hvn_factor` / `lvn_factor` fields from `VrvpEngine`
- [ ] **Step 5** — Add `VrvpProfile::node_at(price) -> NodeType`
- [ ] **Step 6** — Add `VrvpProfile::nearest_hvn_in_direction(price, bullish) -> Option<f64>`
- [ ] **Step 7** — Add `VrvpProfile::nearest_lvn_in_direction(price, bullish) -> Option<f64>`
- [ ] **Step 8** — Add tests: `node_at`, `nearest_hvn_in_direction`, `nearest_lvn_in_direction`, bounded HVN count
- [ ] **Step 9** — Spawn `vrvp_loop` in `main.rs` for 4H (500 candles, 1800s) and 1D (365 candles, 7200s)
- [ ] **Future** — Wire `node_at()` into `structure_score()` for +2 HVN / +1 LVN scoring
- [ ] **Future** — Wire directional nav functions into cascade target derivation (Priority 6)
