use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const WS_PUBLIC_URL: &str = "wss://fapi.bitunix.com/public/";

// ─── WebSocket message structs ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsArg {
    pub symbol: String,
    pub ch: String,
}

// ─── Ticker ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsTickerResponse {
    pub ch: String,
    pub ts: u64,
    pub data: Vec<WsTickerData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsTickerData {
    /// Symbol
    pub s: String,
    /// Last traded price
    pub la: String,
    /// Open price (24h)
    pub o: String,
    /// High price (24h)
    pub h: String,
    /// Low price (24h)
    pub l: String,
    /// Base volume
    pub b: String,
    /// Quote volume
    pub q: String,
    /// Funding rate
    pub r: String,
    /// Best bid price
    pub bd: String,
    /// Best ask price
    pub ak: String,
    /// Best bid volume
    pub bv: String,
    /// Best ask volume
    pub av: String,
}

// ─── Kline ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsKlineResponse {
    pub ch: String,
    pub symbol: String,
    pub data: Vec<WsKlineData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsKlineData {
    pub o: String,
    pub c: String,
    pub h: String,
    pub l: String,
    pub b: String,
    pub q: String,
}

// ─── Channel name helper ─────────────────────────────────────────────────────

/// Convert a human-readable timeframe string to the Bitunix WS channel name.
/// e.g. "1m" → "market_kline_1min", "1h" → "market_kline_60min", "4H" → "market_kline_240min"
pub fn parse_timeframe_to_channel(timeframe: &str) -> Result<String> {
    let ch = match timeframe.to_lowercase().as_str() {
        "1m" => "market_kline_1min",
        "3m" => "market_kline_3min",
        "5m" => "market_kline_5min",
        "15m" => "market_kline_15min",
        "30m" => "market_kline_30min",
        "1h" => "market_kline_60min",
        "2h" => "market_kline_120min",
        "4h" => "market_kline_240min",
        "6h" => "market_kline_360min",
        "12h" => "market_kline_720min",
        "1d" => "market_kline_1day",
        "1w" => "market_kline_1week",
        "1mo" => "market_kline_1month",
        other => {
            return Err(anyhow::anyhow!(
                "Unknown Bitunix timeframe '{}'. Expected one of: 1m 3m 5m 15m 30m 1h 2h 4h 6h 12h 1d 1w 1mo",
                other
            ))
        }
    };
    Ok(ch.to_string())
}

// ─── WS client ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitunixWsClient;

impl BitunixWsClient {
    /// Stream live ticker updates for `symbol` (e.g. "BTCUSDT").
    pub async fn subscribe_tickers(
        symbol: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<WsTickerData>>, Box<dyn std::error::Error>>
    {
        info!("Connecting to Bitunix public WebSocket: {WS_PUBLIC_URL}");

        let (ws_stream, _) = connect_async(WS_PUBLIC_URL).await?;
        let (mut write, mut read) = ws_stream.split();

        let subscribe_msg = json!({
            "op": "subscribe",
            "args": [{ "symbol": symbol, "ch": "tickers" }]
        });

        write
            .send(Message::Text(subscribe_msg.to_string().into()))
            .await?;

        let symbol_owned = symbol.to_string();
        let stream = async_stream::try_stream! {
            let mut last_ping = std::time::Instant::now();
            let ping_interval = std::time::Duration::from_secs(25);

            loop {
                if last_ping.elapsed() >= ping_interval {
                    let ping_ts = chrono::Utc::now().timestamp();
                    let ping_msg = json!({ "op": "ping", "ping": ping_ts });
                    if let Err(e) = write.send(Message::Text(ping_msg.to_string().into())).await {
                        error!("Bitunix WS ping error: {e}");
                        break;
                    }
                    last_ping = std::time::Instant::now();
                }

                let msg_result = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    read.next(),
                )
                .await;

                match msg_result {
                    std::result::Result::Ok(Some(std::result::Result::Ok(Message::Text(text)))) => {
                        // Ignore pong frames
                        if text.contains("\"pong\"") {
                            continue;
                        }

                        match serde_json::from_str::<WsTickerResponse>(&text) {
                            std::result::Result::Ok(resp) => {
                                for ticker in resp.data {
                                    if ticker.s == symbol_owned {
                                        yield ticker;
                                    }
                                }
                            }
                            std::result::Result::Err(_) => {
                                // Non-ticker message (e.g. connect ack), skip
                                continue;
                            }
                        }
                    }
                    std::result::Result::Ok(Some(std::result::Result::Ok(Message::Close(_)))) => break,
                    std::result::Result::Ok(Some(std::result::Result::Err(e))) => {
                        error!("Bitunix ticker WS error: {e}");
                        break;
                    }
                    std::result::Result::Ok(None) => break,
                    std::result::Result::Err(_) => continue, // timeout – check ping next iter
                    _ => continue,
                }
            }
        };

        std::result::Result::Ok(Box::pin(stream))
    }

    /// Stream live kline (candlestick) updates for `symbol` at `timeframe`
    /// (e.g. timeframe = "1m", "4h", "1d").
    #[allow(dead_code)]
    pub async fn subscribe_klines(
        symbol: &str,
        timeframe: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<WsKlineData>>, Box<dyn std::error::Error>>
    {
        let channel = parse_timeframe_to_channel(timeframe).map_err(|e| format!("{e}"))?;

        info!("Connecting to Bitunix public WebSocket for klines ({channel}): {WS_PUBLIC_URL}");

        let (ws_stream, _) = connect_async(WS_PUBLIC_URL).await?;
        let (mut write, mut read) = ws_stream.split();

        let subscribe_msg = json!({
            "op": "subscribe",
            "args": [{ "symbol": symbol, "ch": channel }]
        });

        write
            .send(Message::Text(subscribe_msg.to_string().into()))
            .await?;

        let symbol_owned = symbol.to_string();
        let stream = async_stream::try_stream! {
            let mut last_ping = std::time::Instant::now();
            let ping_interval = std::time::Duration::from_secs(25);

            loop {
                if last_ping.elapsed() >= ping_interval {
                    let ping_ts = chrono::Utc::now().timestamp();
                    let ping_msg = json!({ "op": "ping", "ping": ping_ts });
                    if let Err(e) = write.send(Message::Text(ping_msg.to_string().into())).await {
                        error!("Bitunix kline WS ping error: {e}");
                        break;
                    }
                    last_ping = std::time::Instant::now();
                }

                let msg_result = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    read.next(),
                )
                .await;

                match msg_result {
                    std::result::Result::Ok(Some(std::result::Result::Ok(Message::Text(text)))) => {
                        if text.contains("\"pong\"") {
                            continue;
                        }

                        if let std::result::Result::Ok(resp) =
                            serde_json::from_str::<WsKlineResponse>(&text)
                        {
                            if resp.symbol == symbol_owned {
                                for kline in resp.data {
                                    yield kline;
                                }
                            }
                        }
                    }
                    std::result::Result::Ok(Some(std::result::Result::Ok(Message::Close(_)))) => break,
                    std::result::Result::Ok(Some(std::result::Result::Err(e))) => {
                        error!("Bitunix kline WS error: {e}");
                        break;
                    }
                    std::result::Result::Ok(None) => break,
                    std::result::Result::Err(_) => continue,
                    _ => continue,
                }
            }
        };

        std::result::Result::Ok(Box::pin(stream))
    }
}
