# 8th May Veto Table

## `ConfluenceGate` — Entry Veto Logic

| Condition | Long blocked | Short blocked |
|-----------|-------------|---------------|
| `TrendState=Bearish` AND `RSI=Bearish` | Yes | — |
| `IchimokuCross=KijunBelowSpanB` AND `GC3D=BearIntact` | Yes | — |
| Recent `RegularBearish` or `HiddenBearish` RSI div on **4H** | Yes | — |
| Recent `RegularBearish` or `HiddenBearish` RSI div on **1D** | Yes | — |
| `TrendState=Bullish` AND `RSI=Bullish` | — | Yes |
| `IchimokuCross=KijunAboveSpanB` AND `GC3D=BullIntact` | — | Yes |
| Recent `RegularBullish` or `HiddenBullish` RSI div on **4H** | — | Yes |
| Recent `RegularBullish` or `HiddenBullish` RSI div on **1D** | — | Yes |
| Any signal is `None` (Redis key missing or unreadable) | Never — fail-open | Never — fail-open |

## Veto Design Rules

- **Two-signal requirement for Trend vetoes:** a single bearish Trend signal never blocks alone. Both members of the pair must be present and confirmed. `None` on either member means no veto from that pair.
- **Single-signal sufficient for Strength vetoes:** one bearish divergence event on either 4H or 1D is enough to suppress longs. The two timeframes are independent checks — both are evaluated regardless of the other.
- **Fail-open semantics:** if a Redis key is missing (tracker not yet warmed up, network error, cold start), the signal reads as `None`. `None` never contributes to a veto. The gate allows entry and logs a warning.
- **Size modifier is separate:** veto logic is binary (block or allow). Position sizing is modulated by Trend pillar confirmation count — see size modifier table below.

## Size Modifier Table

Counts how many of the four Trend pillars are actively confirming the trade direction. `None` counts as 0.

| Confirming Trend pillars (of 4) | Modifier | Effect |
|---------------------------------|----------|--------|
| 3–4 | 1.0 | Full size |
| 2 | 0.75 | Slightly reduced |
| 1 | 0.5 | Half size |
| 0 (no veto, all unknown) | 0.25 | Minimum size |

**Trend pillars counted for longs:** `TrendDirection=Bullish`, `RegimeState=Bullish`, `IchimokuCross=KijunAboveSpanB`, `GC3D=BullIntact`

**Trend pillars counted for shorts:** `TrendDirection=Bearish`, `RegimeState=Bearish`, `IchimokuCross=KijunBelowSpanB`, `GC3D=BearIntact`

## Redis Keys Read by the Gate

| Signal | Redis Key | Source module |
|--------|-----------|---------------|
| SMC Trend direction | `trading_bot:trend_state` | `trackers/smart_money_concepts` |
| Weekly RSI regime | `trading_bot:rsi_regime` | `trackers/rsi_regime_tracker` |
| Ichimoku Kijun/SpanB state | `trading_bot:ichimoku_cross` | `trackers/ichimoku` |
| Gaussian Channel 3D regime | `trading_bot:gaussian_regime_3d` | `regime` |
| RSI divergence 4H events | `trading_bot:rsi_div:4H` | `trackers/rsi_divergence_indicator` |
| RSI divergence 1D events | `trading_bot:rsi_div:1D` | `trackers/rsi_divergence_indicator` |