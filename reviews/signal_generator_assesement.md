Overall Assessment
This is one of the better structured trading signal frameworks I've seen codified for a bot. The SST (Strength, Structure, Trend) model mirrors how professional prop desk analysts actually think — the insistence that all three align before sizing up is exactly right.

What I Think Is Genuinely Strong
1. The direction-agnostic design is the most important architectural decision in the document.

Most retail bots hardcode bearish or bullish logic and break when the cycle flips. The entire premise here — that direction is an output of the Trend pillar, not a hardcoded input — is how you build something durable across cycles. The sentence "No price is permanent" is not rhetoric; it's the actual correctness criterion for the system.

2. The macro indicators are well-sourced and non-redundant.

Gaussian Channel 3D → cycle state (bear/bull)
Weekly RSI 43-44 → momentum structural level (NOT overbought/oversold)
Ichimoku Kijun/SpanB cross → distribution warning (distinct from cloud signals)
These three measure different things. That's rare. Most systems stack correlated signals and call it confirmation.

3. The explicit downweighting of MACD weekly (40-56%) is sophisticated and correct.

Most frameworks would include MACD uncritically. Explicitly calling out its poor historical reliability and capping its weight is analytically honest. The 4H RSI trendline is a much better leading signal — the analyst's transcript quotes back this up.

4. The structure scoring rubric (structure_score()) is clean.

The +3/+2/+1 scoring with a minimum threshold of 3 to qualify as a KEY_LEVEL prevents the bot from trading off thin structural pretexts. The VPVR LVN accelerant logic (+1 for nearby LVN) is a nice nuance that most frameworks ignore.

5. The CHOCH vs. confirmed trend flip distinction is critical and handled correctly.

"CHOCH is a warning the prior structure is broken. The trend is not confirmed flipped until CHOCH holds and a second BOS follows in the new direction."

One CHOCH is not a trend flip. Getting this wrong is one of the most common fading mistakes.

Where I Have Reservations or Would Push Back
1. VPVR is computationally non-trivial and currently missing — and it's load-bearing.

VPVR is listed as structure_score +2 (HVN) and +1 (LVN). Without VPVR, many levels that should score 5+ will only score 3. You need a volume profile computation from tick or OHLCV data with volume binned by price level. If you're pulling candle data from Bitget, you can approximate it with a rolling bucket approach — but it needs to be built. This is not a "nice to have" — the cascade target logic breaks without it.

2. The break-and-hold requirement (Archetype 3) has ambiguity at the weekly level.

"2 consecutive closes on the new side AND no weekly close back below the open of the break candle"

Two consecutive closes on what timeframe? If the entry timeframe is 4H, two 4H closes is 8 hours — very different from two daily closes. This needs an explicit hold_timeframe parameter tied to the confirmation timeframe, not the entry timeframe.

3. The LLM Signal Rule Extractor (Priority 7) is the riskiest component.

Using Claude to extract structured trade rules from analyst transcripts is genuinely novel — but the failure mode is silent. If the LLM extracts a plausible-looking but subtly wrong rule (wrong direction dependency, wrong structural role), it writes to trading::signal_rules and the bot acts on it. The schema has approximate_price: null as an escape hatch, but the LLM extractor needs a hard validation step: before writing any extracted rule to Redis, verify that the extracted level is within some tolerance of actual live pivot/zone data. If no live structural match is found, the rule should be quarantined, not written.

4. Weekly RSI 43-44 threshold: the threshold itself may need to drift.

The analyst calls it a "structural RSI level" — and historically it has been around 43-44. But if the next cycle has different duration or volatility characteristics, this number may shift. Hardcoding 43-44 is fine for now, but this should probably be treated as a zone (42-45), not a single line. A 1-point miss will create false regime signals.

5. The Gaussian Channel computation needs parameter specificity.

The document says "Gaussian-kernel-weighted moving average" but doesn't specify the kernel window length or the standard deviation multiplier k. The analyst says 3D chart, which helps, but different k values will produce materially different band widths. You need to match the analyst's exact visual — likely k=2 or k=2.5, window ~50-100 bars. This needs to be pinned explicitly in the implementation.

Implementation Priority Commentary
The document's priorities are correct in order. My one reordering suggestion:

Priority 3 (Ichimoku Kijun/SpanB cross) should be done immediately after Priority 1, because the values are already in Redis — this is a 20-line code change. The cross is already historically significant and the data is sitting unused. Don't wait until after VPVR.

The structural target derivation (Priority 6) is where the bot's actual P&L profile changes most dramatically — arithmetic TPs leave massive money on the table when the structural target is 3x the arithmetic target away. This should be accelerated once TrendState is persisted.

One Critical Gap Not in the Document
There is no mention of how the bot handles a position that is open when the trend state flips.

Archetype 3 says existing rules are invalidated. But what about an open position that is now against the new trend? The bot engine needs an explicit "trend flip while in trade" handler:

If entry_direction != new_trend_direction → close at market or set a trailing stop to next structural level
Do NOT hold a counter-trend position hoping the trend flips back
This is not currently in bot/mod.rs and will cause maximum-drawdown scenarios.

Summary
Component	Status
SST framework design	Excellent
Direction-agnostic architecture	Correct and durable
Macro indicator selection	Well-sourced, non-redundant
Structure scoring rubric	Clean, implementable
VPVR dependency	Missing, load-bearing — build first
MACD downweighting	Analytically correct
CHOCH handling	Correct
Break-and-hold timeframe ambiguity	Needs clarification
LLM extractor validation	Needs quarantine logic
Trend flip / open position handler	Missing from bot loop
The document is ready to drive implementation. The two things to resolve before writing production code are: (1) the VPVR computation strategy, and (2) the open-position-on-trend-flip handler. Everything else is well-specified enough to build directly from this doc.