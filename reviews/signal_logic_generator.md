# Signal Logic Generator — Analysis & Implementation Plan

---

## Origin of This Framework

The SST (Strength, Structure, Trend) framework used throughout this document was not invented
here — it was extracted directly from the analyst transcripts in `btc.txt`. The analyst uses
this exact terminology:

> *"When looking for macro trend reversals, confirmation is required before action. The three
> metrics are: Strength (momentum), Structure (sellside liquidity), and Trend (downtrend
> direction). All three must turn positive to have high confidence that the trend will reverse
> and sustain for multiple weeks or months."*
>
> *"For confirmed reversals on the high time frame, all three must be ticked off. For
> probabilistic shifts, at least two. With only one, it is essentially a gamble — not
> impossible, but betting against the probabilities."*

This is the vocabulary and decision logic the signal generator must implement.

---

## Core Premise

The Signal Logic Generator translates analyst context into executable trade decisions. It does
not predict the future — it reads the current state of three variables and determines when all
three align enough to act.

**No price is permanent.** A level that was resistance becomes support when the trend flips.
A bearish signal rule becomes a bullish one when SST flips direction. The generator cannot
hardcode levels, directions, or targets. Everything must be derived from the three living inputs:

```
S — Strength:  Does the current momentum confirm the expected direction?
S — Structure: Which levels carry structural weight, and on which side?
T — Trend:     What is the dominant direction on the relevant timeframe?
```

A signal fires when all three align. When one changes — trend flips, structure breaks, momentum
diverges — the active rules update accordingly.

---

## Part 1: The Three Pillars in Detail

### Pillar 1: Trend

Trend is the dominant direction of price on the timeframe *above* the entry timeframe.

- Entry on 4H → confirm trend on Daily or 3-Day
- Entry on 1H → confirm trend on 4H
- Macro regime (weekly) is the overriding filter above all

**Trend is confirmed by market structure, not assumed:**

- **Uptrend**: sequence of Higher Highs (HH) and Higher Lows (HL). Each `BullishBOS` from the
  `SmcEngine` advances the uptrend. A swing low holding above the prior swing low confirms HL.
- **Downtrend**: sequence of Lower Highs (LH) and Lower Lows (LL). Each `BearishBOS` advances
  the downtrend. A swing high failing below the prior confirms LH.
- **Change of Character (CHOCH)**: a BOS in the *opposite* direction. Not yet confirmation of
  the new trend — it is a warning the prior structure is broken. The trend is not confirmed
  flipped until the CHOCH holds and a second BOS follows in the new direction.

**Macro trend indicators used in the transcripts (and what they each confirm):**

**1. Gaussian Channel — 3-Day Chart (primary bear market end signal)**

The analyst explicitly names this as the primary macro reversal indicator:

> *"While we are underneath the Gaussian channel on the 3-day, the bear market is intact.
> When we break above the Gaussian channel on the 3-day, historically the bear market ends."*

Four for four historical occurrences across 2015, 2019, 2022, and 2011 cycles. The break above
the lower band of the Gaussian Channel on the 3-day chart is the bear-market-end confirmation.
The break below the upper band on the weekly is the bear-market-start signal.

Regime states from the Gaussian Channel:
- `close > upper_band` (3D): bull market confirmed — buy-the-dip regime
- `close < lower_band` (3D): bear market intact — sell-the-rally regime
- `inside_channel` (3D): transitioning — stand aside or reduce size

**2. Gaussian Channel — Weekly Chart (volatile leg signal)**

> *"We've used it on the weekly perspective for periods of mass volatility, such as the
> breakdown of the Gaussian channel lower band leading into the beginning of 2026, which
> triggered that massive volatile correction."*

Breaking the weekly Gaussian Channel lower band → imminent high-volatility leg down.
Reclaiming the weekly Gaussian Channel upper band → beginning of bull market cycle.

**3. Weekly RSI level at 43–44 (macro trend threshold)**

Explicitly named in the transcripts as a structural RSI level:

> *"Historically, when we break the bear market, two key things occur: 1) a structural
> sell-side liquidity point is reclaimed. 2) A breakout occurs on a key horizontal level RSI —
> historically around 43–44 on the weekly."*
>
> *"We broke above that RSI level previously and that confirmed the macro reversal from the
> bear market ending and the bull market beginning. We remained above that level the entire
> bull run. Only when we broke down did we enter the bear market. Currently we are below
> that RSI level. While we remain below it, we are in a bear market."*

This is not an overbought/oversold threshold — it is a structural level on the RSI itself. The
weekly RSI 43–44 zone is a market structure level on the momentum indicator, equivalent to
what pivot detection finds on price.

**4. Ichimoku Baseline crossing below Leading Span B (macro correction trigger)**

The analyst is specific about which Ichimoku cross matters:

> *"Very recently, the baseline has crossed below the leading span B. Looking back across the
> past 12 years, this has occurred four times. Each time, this has been a major trigger point
> for extended corrections."*
>
> From each occurrence: 74% drop, 50% drop, 50% drop.

The cross to watch is **Kijun-Sen (baseline, 26-period) crossing below Senkou Span B
(52-period)** on the weekly chart. This is not a Tenkan/Kijun cross — it is a specific
base/cloud relationship that signals macro distribution.

**5. MACD Weekly — Explicitly Low Reliability**

The transcripts contain a detailed probability analysis:
> *"A 56% success rate for a macro momentum indicator is poor."*
> *"Bullish cross while momentum is NEGATIVE (below zero) — 10 instances: 4 of 10 resulted
> in continuation upwards = 40% success rate."*

**The MACD weekly cross should carry very low weight in signal decisions.** It is explicitly
described as insufficient to confirm macro reversals. RSI and Ichimoku have historically shown
probabilities above 70% on equivalent signals. The signal generator should not weight MACD
crosses heavily, especially when the MACD histogram is below zero.

**Trend State — what to persist in Redis:**

```
TrendState {
    // Entry timeframe
    entry_tf_direction:       Bullish | Bearish | Neutral,
    last_bos_level:           f64,       // last BOS price — structural, not hardcoded
    last_bos_direction:       Bullish | Bearish,
    choch_detected:           bool,

    // Confirmation timeframe (one above entry)
    confirm_tf_direction:     Bullish | Bearish | Neutral,

    // Macro regime
    gaussian_3d_state:        BullIntact | BearIntact | Transitioning,
    gaussian_weekly_band:     AboveUpper | BelowLower | Inside,
    weekly_rsi_level:         AboveThreshold | BelowThreshold,   // threshold = 43-44
    ichimoku_cross_bearish:   bool,    // baseline crossed below span B

    last_updated:             timestamp
}
```

**When trend flips:** all pending signal rules from the prior trend direction are invalidated.
New rules are generated from the current structure in the new direction.

---

### Pillar 2: Structure

Structure is the map of why a price level matters. Not every level is a signal — only levels
that carry structural weight generate valid triggers or targets.

**Primary structural identification tools (from the transcripts):**

**1. VPVR (Volume Profile Visible Range)**

The analyst uses VPVR extensively as the primary structural tool:

> *"VPVR shows a gap in this region, suggesting it will act as a support point."*
> *"Massive low historical volume ranges sit above 72.2K and below 66,000."*
> *"A loss of 66,000 will result in a massive amount of volatility toward the lower support."*

VPVR identifies *why* a level is significant: low-volume nodes cause rapid price movement (gaps
= volatility), high-volume nodes cause slow grinding or rejection. A structural level is
reinforced when it coincides with a VPVR high-volume node (HVN) for support or a low-volume
node (LVN) that sits just above/below, which would accelerate a move once the level breaks.

**2. SMC Pivot Highs / Pivot Lows**

Where price reversed previously. The higher the timeframe, the more significant. These are the
`PivotHigh` and `PivotLow` events from `SmcEngine`.

**3. BOS Levels**

Where price previously broke structure. These levels often flip — former support becomes
resistance (and vice versa) after a BOS.

**4. Liquidity Sweep Levels (Sellside/Buyside)**

Where price grabbed stops above a swing high or below a swing low before reversing.
`SweepHigh` and `SweepLow` events from `SmcEngine`. The analyst refers to these as "sellside
liquidity" and "buyside liquidity" pools.

**5. EMA Clusters (50 EMA, 200 EMA)**

The analyst uses 50 EMA and 200 EMA as dynamic support/resistance levels and mean reversion
targets:
> *"Expect a potential pullback toward the 50 EMA and 200 EMA, or a break of structure level."*

These are not fixed levels — they track with price. They serve as the "expected landing zone"
for pullbacks, not primary structural levels.

**Structure quality scoring:**

```
structure_score(level) =
    +3  if level == major swing high/low visible on weekly or daily
    +2  if level == prior BOS level (flipped support/resistance)
    +2  if level == VPVR high-volume node (HVN) — acts as magnet or brake
    +1  if level == VPVR low-volume node (LVN) nearby — accelerates moves through it
    +1  if level == liquidity sweep level (sellside/buyside pool)
    +1  if level == EMA confluence (50 or 200 EMA nearby on the same bar)
    +2  per additional timeframe that identifies the same level as significant

→ minimum threshold to qualify as KEY_LEVEL: score >= 3
→ levels scoring >= 5 are high-conviction structural levels (full position size)
```

**Targets are structural, not arithmetic:**

When a level breaks in direction D, the first target is the next structurally significant level
in direction D. The second target is the level beyond that. These are read from the live pivot
history and zone data — they change as structure evolves.

The cascade pattern from the transcripts shows how this works in practice:
> *"Below 65,800: high probability of move toward 62,300, and more likely 60,000."*
> *"60,000: High time frame trigger on the daily for a correction toward 52k."*

65,800 → 62,300 → 60,000 → 52,000: each level is the next structurally scored level in the
direction of the break. None of these numbers are arbitrary — each is a VPVR node, a prior
pivot, or a BOS level. The signal generator must derive this cascade from the live structure map,
not from the analyst's stated numbers (which will be different in the next market cycle).

---

### Pillar 3: Strength

Strength is the evidence that the current move has conviction and is not a false break.

The transcripts are specific about the distinction:

> *"When looking at indicators such as the RSI during periods of mass volatility followed by
> strong consolidation, momentum indicators will start to curve upwards. This does not
> necessarily mean there is strength — it means there is a decrease in selling pressure,
> which is not the same as buying strength. These two must be clearly differentiated."*

**Decreasing selling pressure ≠ buying strength.** The signal generator must distinguish between:
- RSI rising from oversold → **decreasing negative momentum** (weak positive signal)
- RSI breaking above its trendline on the 4H + volume expansion → **active buying strength**
  (strong positive signal)

**Strength indicators from the transcripts:**

| Signal | Measures | How the analyst uses it |
|--------|---------|------------------------|
| RSI trendline break (4H) | Momentum direction change | Leading signal for trend change — "the 4-hour uptrend on the RSI finally broke down" |
| RSI above/below its prior lows (matching lows) | Divergence detection | "matching lows on the RSI aligned with price action" = the RSI uptrend is intact |
| VPVR low-volume node on break candle | Conviction — move will continue | "A loss of 66,000 will result in a massive amount of volatility" = LVN below acts as accelerant |
| Volume ratio on break candle | Participation | "Liquidations are up 87% at $525M" = high-conviction move |
| Candle body size at break | Decisive close through level | Wicks through a level are explicitly called out as weaker than body closes |
| No immediate rejection after break | Sustained move | "remains at or below it, consolidation continues" = hold, not just touch |

**Strength is a soft filter:**
- High strength + all three SST aligned → full position size
- Moderate strength + two of three SST aligned → reduced position size
- Low strength OR only one SST aligned → no high-timeframe trade (only range scalps)

---

## Part 2: The Four Signal Archetypes — Direction-Agnostic Templates

The analyst's specific rules all reduce to these four templates. Direction, levels, and targets
are variable inputs. The template logic is invariant.

---

### Archetype 1: Level Break With Timeframe Confirmation

This is the primary trade entry pattern:

```
PRECONDITIONS (the gate — all three must pass):

  TREND:     trend_state.entry_tf_direction    == DIRECTION
             trend_state.confirm_tf_direction  == DIRECTION
             gaussian_3d_state                != opposing_regime
             weekly_rsi_level                 == regime_aligned(DIRECTION)

  STRUCTURE: structure_score(KEY_LEVEL) >= 3
             KEY_LEVEL is on the correct side (resistance for shorts, support for longs)

  STRENGTH:  volume_ratio on break candle >= 1.5x average
             rsi_4H trending in DIRECTION at time of break
             no major divergence printing at KEY_LEVEL

TRIGGER:
  candle[TIMEFRAME].close crosses KEY_LEVEL in DIRECTION
  AND previous candle close was on the opposite side
  (wick does not count — only a confirmed close beyond the level)

ACTION:
  enter DIRECTION at market
  target_1 = next_scored_level(DIRECTION, from=KEY_LEVEL)    // structural, not hardcoded
  target_2 = scored_level_beyond(target_1, in=DIRECTION)
  stop_loss = last_structural_extreme_opposite(DIRECTION)
              // last swing high for shorts, last swing low for longs

  position_size = function(strength_score, sst_alignment_count)
                  // full size only when all three pillars strong
```

**When the market is bearish (current state as of transcripts):**
KEY_LEVEL is a prior structural high acting as resistance. Break is downward. Targets are the
next scored structural supports below. Example from transcripts: 65.8K breaks → targets 62.3K
→ 60K.

**When the market flips bullish:**
KEY_LEVEL is a prior structural low acting as support. Break is upward. Targets are the next
scored resistances above. Same template, opposite direction. Example: reclaim of 74-78K zone
on break-and-hold → targets are the next structural highs above that zone.

The template is identical. Direction is an output of the Trend pillar.

---

### Archetype 2: RSI Trendline Break (Momentum Leading Signal)

The RSI forms its own structure. Its swing highs and lows define trendlines. When those
trendlines break, a momentum shift is signaled — before price confirms it.

The analyst uses this explicitly:

> *"The 4-hour uptrend on the RSI finally broke down. After around two and a bit weeks of
> gradually increasing momentum, that momentum has now officially turned negative."*
> *"If the RSI trend line breaks and comes back down, that is the sell signal."*
> *"The most important thing: we are continuing to close 4-hour candles underneath 72,200.
> Provided we continue... momentum will continue to fall, potentially resulting in a negative
> momentum shift."*

```
INPUTS:
  rsi_value_series (computed on entry timeframe, period=14)

STRUCTURAL DETECTION:
  apply pivot detection (left=3, right=3) to rsi_value_series
  → find rsi_pivot_highs (for bearish trendline)
  → find rsi_pivot_lows  (for bullish trendline)

TRENDLINE CONSTRUCTION:
  bearish_trendline = line connecting last_2_rsi_pivot_highs
  bullish_trendline = line connecting last_2_rsi_pivot_lows
  project both to current bar_index

TRIGGER (bearish momentum break):
  rsi_current < bearish_trendline_value AND rsi_previous >= bearish_trendline_value
  → emit: RsiMomentumBreak { direction: Bearish, rsi_value, timeframe, bar_time }

TRIGGER (bullish momentum break):
  rsi_current > bullish_trendline_value AND rsi_previous <= bullish_trendline_value
  → emit: RsiMomentumBreak { direction: Bullish, rsi_value, timeframe, bar_time }

EFFECT ON TRADING:
  RsiMomentumBreak::Bearish →
    - suppress new long entries
    - prepare for Archetype 1 short trigger at next KEY_LEVEL below current price
    - reduce size on any existing long positions

  RsiMomentumBreak::Bullish →
    - suppress new short entries
    - prepare for Archetype 1 long trigger at next KEY_LEVEL above current price
    - reduce size on any existing short positions
```

**Divergence (a stronger version of this signal):**
When RSI makes matching lows (equal lows) while price makes a new low → RSI trendline intact,
divergence developing → do NOT enter new shorts. Wait for RSI trendline to actually break.

When RSI makes a lower high while price makes a higher high → hidden bearish divergence →
RSI momentum already breaking before price does.

---

### Archetype 3: Break-and-Hold → Trend State Change

A break-and-hold is the event that updates the Trend pillar. It is what flips the entire signal
ruleset from one direction to the other.

From the transcripts, the specific requirement:

> *"Break and hold above 74-78K → macro downtrend invalidated."*
> *"The $78,000 level is the invalidation of this historical pattern. A sustained breakout
> above 78,000 would begin to invalidate the 4-year cycle bear market structure."*

The STRUCTURAL_ZONE is always derived from the current scored structure. Its specific price
coordinates change from cycle to cycle. What does not change is the definition: it is the major
structural resistance/support that, if broken and held, confirms the trend has changed.

```
IDENTIFICATION:
  STRUCTURAL_ZONE = zone with highest structure_score in the opposing direction
                    that has NOT yet been broken (the defining level of the current trend)

TRIGGER:
  price closes through STRUCTURAL_ZONE in the new direction
  AND holds for >= 2 consecutive closes on the new side
  AND no weekly close back below the open of the break candle

EFFECT:
  trend_state.choch_detected = true                  // trend potentially flipping
  IF subsequent BOS follows in the new direction:
    trend_state.entry_tf_direction = NEW_DIRECTION
    trend_state.confirm_tf_direction = re-evaluate on higher TF
    ALL signal rules with trend_dependency = PRIOR_DIRECTION → INVALIDATED
    → regenerate KEY_LEVEL candidates from new structural map
    → regenerate cascading targets from new structural direction

WHAT IT IS NOT:
  A single close above the zone is not a break-and-hold.
  A wick above without a body close is not a break-and-hold.
  The analyst explicitly distinguishes: "a break above this level" vs "a sustained close
  above this level" — only the latter triggers the trend change.
```

---

### Archetype 4: Regime State Condition (Persistent Filter)

The regime filter gates all trade entries. It does not generate trades — it classifies the
environment and determines trade direction weighting.

**Two layers from the transcripts:**

**Layer 1 — Gaussian Channel 3-Day (bear/bull market cycle)**
```
COMPUTATION:
  gaussian_ma[i]   = Gaussian-kernel-weighted moving average of close[i]
  gaussian_upper   = gaussian_ma + k * gaussian_stdev
  gaussian_lower   = gaussian_ma - k * gaussian_stdev
  (computed on 3D candles via get_bitget_candles("3d", ...))

REGIME_STATE:
  if close > gaussian_upper:  BullMarketIntact    // buy dips, counter-trend shorts risky
  if close < gaussian_lower:  BearMarketIntact    // sell rallies, counter-trend longs risky
  if inside channel:          Transitioning       // reduce size, wait for clarity

GATE EFFECT:
  BearMarketIntact:  short entries = trend-aligned (full size)
                     long entries  = counter-trend (reduce size or skip high-TF)
  BullMarketIntact:  long entries  = trend-aligned (full size)
                     short entries = counter-trend (reduce size or skip high-TF)
  Transitioning:     all entries reduced size
```

**Layer 2 — Weekly RSI structural threshold at 43–44**
```
WEEKLY_RSI_STATE:
  if weekly_rsi > 43:  above_threshold   // macro momentum recovering, bull market possible
  if weekly_rsi < 43:  below_threshold   // macro bear momentum intact

GATE EFFECT (modifier, not veto):
  below_threshold + BearMarketIntact → strong confirmation: bearish entries get +1 to sizing
  above_threshold + BullMarketIntact → strong confirmation: bullish entries get +1 to sizing
  signals that conflict with RSI threshold state → reduce size by half
```

**Layer 3 — Ichimoku Kijun-Sen crossing below Senkou Span B (weekly)**
```
CROSS_DETECTION:
  if kijun_previous >= span_b_previous AND kijun_current < span_b_current:
    emit IchimokuBearishCross { timeframe: "1W" }   // major macro correction incoming

GATE EFFECT:
  IchimokuBearishCross active → major downside risk elevated
  suppress counter-trend longs on high-TF
  reduce position sizing across all entries until cross resolves
```

---

## Part 3: What Already Exists in btc_trading_bot

### SMC Engine — [src/trackers/smart_money_concepts/mod.rs](../btc_trading_bot/src/trackers/smart_money_concepts/mod.rs)

Produces `PivotHigh`, `PivotLow`, `SweepHigh`, `SweepLow`, `BullishBOS`, `BearishBOS`,
`StrongLow`, `StrongHigh`.

**Maps to:**
- Structure pillar: pivot events → `structure_score()` inputs
- Trend pillar: last BOS direction → `trend_state.entry_tf_direction`
- Archetype 1 trigger: `BearishBOS` / `BullishBOS` close through KEY_LEVEL

**Missing:**
- `TrendState` object is not persisted to Redis — bot does not know the current trend direction
- Structure scoring (`structure_score()` function) does not exist — needs to be built using
  pivot and BOS history
- VPVR data is not available — would need a separate Volume Profile computation

---

### Momentum Tracker — [src/trackers/momentum/mod.rs](../btc_trading_bot/src/trackers/momentum/mod.rs)

Computes RSI(14), MACD, price momentum, volume ratio.

**Maps to:**
- Strength pillar: volume ratio, RSI direction
- Archetype 2: RSI values are the input series

**Missing:**
- RSI trendline detection — RSI pivot detection + trendline projection (needs to be added)
- MACD signal line is simplified (`signal = macd * 0.8` instead of true 9-period EMA) — and
  per the transcripts, MACD should carry low weight anyway (40-56% reliability)
- No RSI divergence detection (matching lows, hidden bearish divergence)

---

### Ichimoku Tracker — [src/trackers/ichimoku/mod.rs](../btc_trading_bot/src/trackers/ichimoku/mod.rs)

Computes the full weekly Ichimoku Cloud. Stores last 25 Span A/B values in Redis.

**Maps to:**
- Archetype 4, Layer 3: Kijun-Sen / Span B cross detection

**Missing:**
- The cross detection logic (`kijun < span_b`) is not implemented — the values are computed
  and stored, but the cross event is never emitted or acted on
- The bot's main loop does not read Ichimoku data to gate entries

---

### Bot Engine — [src/bot/mod.rs](../btc_trading_bot/src/bot/mod.rs)

Has `partial_profit_target` Vec, `ZoneGuard`, `prepare_open_position()`.

**Maps to:**
- Archetype 1 action: `partial_profit_target` is the cascading target infrastructure
- Archetype 3 hold check: `ZoneGuard` filters zones that repeatedly fail (proxy for no-hold)
- Position sizing: `prepare_open_position()` calculates size from margin, leverage, risk %

**Missing:**
- No SST gate before entry — bot enters zones regardless of trend alignment
- No structural target derivation — targets are arithmetic, not derived from next scored levels
- No `TrendState` check anywhere in the entry loop

---

### LLM Sentiment — [src/trackers/llm_sentiment/sentiment.rs](../btc_trading_bot/src/trackers/llm_sentiment/sentiment.rs)

Currently dormant. Returns a 3-class label.

**Maps to:**
- The transcript ingestion layer — but currently not connected to structured rule extraction

---

## Part 4: Implementation Priorities

### Priority 1 — Persist TrendState from SMC Events

The `SmcEngine` already emits `BullishBOS` and `BearishBOS`. Persist the last BOS direction
as `TrendState.entry_tf_direction` in Redis. Read it in the bot loop before entering.

This is the **highest-leverage single change** — it makes every existing signal trend-aware.

### Priority 2 — Wire Gaussian Channel 3D + Weekly as Regime Filters

New tracker: `src/trackers/gaussian_channel/mod.rs`. Fetch 3D candles via
`get_bitget_candles("3d", ...)`. Compute Gaussian kernel weights, midline, upper/lower bands.
Emit `RegimeState` to Redis. Bot reads this before any entry.

Separately, emit the weekly Gaussian band crossing as a `VolatilityAlert`.

### Priority 3 — Wire Ichimoku Kijun/SpanB Cross Detection

The values are already in Redis. Add the cross detection: when Kijun-Sen crosses below
Senkou Span B on the weekly, set `ichimoku_cross_bearish = true` in `TrendState`. Bot reads
this as a high-conviction bearish gate.

### Priority 4 — RSI Trendline Break Detector

New tracker module. Apply `SmcEngine` pivot logic to the RSI value series. Emit
`RsiMomentumBreak { direction }` to Redis. Bot reads this before entering counter-direction
trades.

### Priority 5 — Weekly RSI Threshold State

Compute weekly RSI (14-period on weekly candles). Compare against 43-44 threshold. Persist
state to Redis. Bot applies as a macro directional filter.

### Priority 6 — Structural Target Derivation

Replace arithmetic TP calculation with structural lookup: when entering a trade, `target_1`
= next scored pivot/zone in the trade direction, `target_2` = the one after. Read from live
Redis zone data.

### Priority 7 — LLM Signal Rule Extractor

Evolve `SentimentClient` into a `SignalRuleExtractor` calling the Claude API. Extract
structured rules from transcripts using the schema below. Validate against live structure
before writing to Redis.

---

## Part 5: LLM Extraction Schema

When the LLM parses a transcript, it extracts rules into this direction-agnostic schema:

```json
{
  "rules": [{
    "archetype": "level_break" | "momentum_break" | "break_and_hold" | "regime_state",
    "direction": "long" | "short" | "trend_flip_bullish" | "trend_flip_bearish",
    "trend_dependency": "bearish_trend" | "bullish_trend" | "any",
    "trigger": {
      "level_role": "resistance" | "support" | "bos_level" | "sweep_level",
      "structural_significance": "high" | "medium",
      "timeframe": "1H" | "4H" | "1D" | "3D" | "1W",
      "confirmation": "candle_close" | "hold_n_candles",
      "hold_count": 1,
      "approximate_price": null
    },
    "targets": {
      "type": "structural",
      "derivation": "next_scored_level_in_direction"
    },
    "invalidation": {
      "level_role": "structural_extreme_opposite_to_trade",
      "approximate_price": null
    },
    "sst_requirements": {
      "strength_min": "rsi_aligned" | "volume_expanded" | "both",
      "structure_min_score": 3,
      "trend_required": true
    },
    "confidence": "high" | "medium" | "low"
  }]
}
```

`approximate_price` is populated only as a search hint — the system uses it to find nearby
scored structural levels, then uses those levels as the actual trigger, not the raw price.

---

## Part 6: Data Flow

```
Analyst Transcripts (btc.txt)
        │
        ▼
LLM Signal Rule Extractor (Claude API)
  → direction-agnostic templates with structural hints
        │
        ▼
Redis: trading::signal_rules
        │
        │
        ├──────────────────────────────────────────────────────┐
        ▼                                                      ▼
SMC Engine (async loop)                          Momentum Tracker (async loop)
  ├─ PivotHigh / PivotLow                          ├─ RSI(14) — value series
  ├─ SweepHigh / SweepLow                          ├─ RSI trendline break detection
  ├─ BullishBOS / BearishBOS ──────────────────►   ├─ Volume ratio
  └─ StrongLow / StrongHigh                        └─ RsiMomentumBreak events
        │                                                      │
        ▼                                                      ▼
Redis: trading::zones                       Redis: trading::momentum_state
Redis: trading::trend_state                 Redis: trading::rsi_trendline_break
        │
        ▼
Ichimoku Tracker (weekly)                   Gaussian Channel Tracker
  ├─ Kijun-Sen / Span B values                ├─ 3D candles: midline, upper, lower
  └─ Cross detection                          ├─ RegimeState (BullIntact/BearIntact/Transit)
        │                                     └─ Weekly band crossing (VolatilityAlert)
        ▼                                                      │
Redis: trading::ichimoku_cross              Redis: trading::gaussian_regime
        │                                                      │
        └──────────────────────────────────────────────────────┘
                                │
                       all feeds into
                                │
                                ▼
                    Bot Main Loop (bot/mod.rs)
                                │
                ┌───────────────┼───────────────────┐
                ▼               ▼                   ▼
          TREND GATE      STRUCTURE GATE      STRENGTH GATE
         TrendState       structure_score()   volume_ratio
         Gaussian 3D      KEY_LEVEL >= 3      RSI trendline
         Weekly RSI       zone alignment      no divergence
         Ichimoku cross   VPVR context        candle body
                │               │                   │
                └───────────────┴───────────────────┘
                                │
                  ALL THREE MUST PASS (or 2 for reduced size)
                                │
                                ▼
                   Entry Decision (Long / Short)
                   target_1 = next_scored_level(direction)
                   target_2 = scored_level_beyond(target_1)
                   stop_loss = last_structural_extreme_opposite
                                │
                                ▼
                     Bitget Exchange (order execution)
                                │
                                ▼
                      Redis: trading::active
                      Redis: trading::closed_positions
```
