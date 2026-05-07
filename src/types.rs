#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    pub timestamp: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Deserialize)]
pub struct RawMinuteRow {
    #[serde(rename = "Timestamp")]
    pub timestamp: f64,
    #[serde(rename = "Open")]
    pub open: Option<f64>,
    #[serde(rename = "High")]
    pub high: Option<f64>,
    #[serde(rename = "Low")]
    pub low: Option<f64>,
    #[serde(rename = "Close")]
    pub close: Option<f64>,
    #[serde(rename = "Volume")]
    pub volume_btc: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    M15,
    M30,
    H1,
    H4,
    H12,
    D1,
    D3,
    W1,
    W2,
    Monthly,
}

impl Timeframe {
    pub fn seconds(&self) -> i64 {
        match self {
            Timeframe::M15 => 15 * 60,
            Timeframe::M30 => 30 * 60,
            Timeframe::H1 => 60 * 60,
            Timeframe::H4 => 4 * 60 * 60,
            Timeframe::H12 => 12 * 60 * 60,
            Timeframe::D1 => 24 * 60 * 60,
            Timeframe::D3 => 3 * 24 * 60 * 60,
            Timeframe::W1 => 7 * 24 * 60 * 60,
            Timeframe::W2 => 14 * 24 * 60 * 60,
            Timeframe::Monthly => 30 * 24 * 60 * 60,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Timeframe::M15 => "15m",
            Timeframe::M30 => "30m",
            Timeframe::H1 => "1h",
            Timeframe::H4 => "4h",
            Timeframe::H12 => "12h",
            Timeframe::D1 => "1d",
            Timeframe::D3 => "3d",
            Timeframe::W1 => "1w",
            Timeframe::W2 => "2w",
            Timeframe::Monthly => "1M",
        }
    }
}
