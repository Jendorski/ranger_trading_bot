use std::collections::VecDeque;
use std::result::Result::Ok;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use log::info;

use crate::exchange::bitget::{Candle, CandleData, HttpCandleData};

#[derive(Debug, Clone)]
pub struct PriceData {
    pub price: f64,
    pub volume: f64,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct MACDData {
    pub macd: f64,
    pub signal: f64,
    pub histogram: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MomentumSignal {
    Bullish,
    Bearish,
    Neutral,
}

#[derive(Debug, Clone)]
pub struct MomentumIndicators {
    pub rsi: f64,
    pub macd: MACDData,
    pub price_momentum: f64,
    pub volume_ratio: f64,
    pub overall_signal: MomentumSignal,
}

pub struct BitcoinMomentumTracker {
    price_history: VecDeque<f64>,
    volume_history: VecDeque<f64>,
    timestamps: VecDeque<u64>,
    max_history: usize,
}

impl BitcoinMomentumTracker {
    /// Creates a new momentum tracker with specified history limit
    pub fn new(max_history: usize) -> Self {
        Self {
            price_history: VecDeque::with_capacity(max_history),
            volume_history: VecDeque::with_capacity(max_history),
            timestamps: VecDeque::with_capacity(max_history),
            max_history,
        }
    }

    /// Adds new price data point
    pub fn add_data_point(&mut self, price: f64, volume: f64) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.price_history.push_back(price);
        self.volume_history.push_back(volume);
        self.timestamps.push_back(timestamp);

        // Maintain max history limit
        while self.price_history.len() > self.max_history {
            self.price_history.pop_front();
            self.volume_history.pop_front();
            self.timestamps.pop_front();
        }
    }

    /// Calculates RSI (Relative Strength Index)
    pub fn calculate_rsi(&self, period: usize) -> Option<f64> {
        if self.price_history.len() < period + 1 {
            return None;
        }

        let prices: Vec<f64> = self.price_history.iter().cloned().collect();
        let mut gains = 0.0;
        let mut losses = 0.0;

        for i in (prices.len() - period)..prices.len() {
            let change = prices[i] - prices[i - 1];
            if change > 0.0 {
                gains += change;
            } else {
                losses -= change;
            }
        }

        let avg_gain = gains / period as f64;
        let avg_loss = losses / period as f64;

        if avg_loss == 0.0 {
            return Some(100.0);
        }

        let rs = avg_gain / avg_loss;
        Some(100.0 - (100.0 / (1.0 + rs)))
    }

    /// Calculates Exponential Moving Average
    pub fn calculate_ema(&self, prices: &[f64], period: usize) -> Option<f64> {
        if prices.is_empty() {
            return None;
        }

        let multiplier = 2.0 / (period as f64 + 1.0);
        let mut ema = prices[0];

        for &price in prices.iter().skip(1) {
            ema = (price * multiplier) + (ema * (1.0 - multiplier));
        }

        Some(ema)
    }

    /// Calculates MACD (Moving Average Convergence Divergence)
    pub fn calculate_macd(&self) -> Option<MACDData> {
        if self.price_history.len() < 26 {
            return None;
        }

        let prices: Vec<f64> = self.price_history.iter().cloned().collect();

        let ema12 = self.calculate_ema(&prices, 12)?;
        let ema26 = self.calculate_ema(&prices, 26)?;
        let macd = ema12 - ema26;

        // Simplified signal line calculation
        let signal = macd * 0.8;
        let histogram = macd - signal;

        Some(MACDData {
            macd,
            signal,
            histogram,
        })
    }

    /// Calculates price momentum over specified period
    pub fn calculate_price_momentum(&self, period: usize) -> Option<f64> {
        if self.price_history.len() < period {
            return None;
        }

        let current_price = *self.price_history.back()?;
        let previous_price = self.price_history[self.price_history.len() - period];

        Some(((current_price - previous_price) / previous_price) * 100.0)
    }

    /// Calculates volume ratio compared to average
    pub fn calculate_volume_ratio(&self) -> Option<f64> {
        if self.volume_history.is_empty() {
            return None;
        }

        let current_volume = *self.volume_history.back()?;
        let avg_volume: f64 =
            self.volume_history.iter().sum::<f64>() / self.volume_history.len() as f64;

        if avg_volume == 0.0 {
            return Some(1.0);
        }

        Some(current_volume / avg_volume)
    }

    /// Determines momentum signal based on RSI
    pub fn get_rsi_signal(&self, rsi: f64) -> MomentumSignal {
        if rsi > 70.0 {
            MomentumSignal::Bearish // Overbought
        } else if rsi < 30.0 {
            MomentumSignal::Bullish // Oversold
        } else {
            MomentumSignal::Neutral
        }
    }

    /// Determines momentum signal based on MACD
    pub fn get_macd_signal(&self, macd: &MACDData) -> MomentumSignal {
        if macd.histogram > 0.0 {
            MomentumSignal::Bullish
        } else if macd.histogram < 0.0 {
            MomentumSignal::Bearish
        } else {
            MomentumSignal::Neutral
        }
    }

    /// Determines momentum signal based on price movement
    pub fn get_price_momentum_signal(&self, momentum: f64) -> MomentumSignal {
        if momentum > 1.0 {
            MomentumSignal::Bullish
        } else if momentum < -1.0 {
            MomentumSignal::Bearish
        } else {
            MomentumSignal::Neutral
        }
    }

    /// Calculates overall momentum signal by combining all indicators
    pub fn get_overall_momentum_signal(&self, indicators: &MomentumIndicators) -> MomentumSignal {
        let rsi_signal = self.get_rsi_signal(indicators.rsi);
        let macd_signal = self.get_macd_signal(&indicators.macd);
        let price_signal = self.get_price_momentum_signal(indicators.price_momentum);

        // Count bullish and bearish signals
        let mut bullish_count = 0;
        let mut bearish_count = 0;

        for signal in [&rsi_signal, &macd_signal, &price_signal] {
            match signal {
                MomentumSignal::Bullish => bullish_count += 1,
                MomentumSignal::Bearish => bearish_count += 1,
                MomentumSignal::Neutral => {}
            }
        }

        if bullish_count > bearish_count {
            MomentumSignal::Bullish
        } else if bearish_count > bullish_count {
            MomentumSignal::Bearish
        } else {
            MomentumSignal::Neutral
        }
    }

    /// Calculates all momentum indicators
    pub fn calculate_all_indicators(&self) -> Option<MomentumIndicators> {
        let rsi = self.calculate_rsi(14).unwrap_or(50.0);
        let macd = self.calculate_macd().unwrap_or(MACDData {
            macd: 0.0,
            signal: 0.0,
            histogram: 0.0,
        });
        let price_momentum = self.calculate_price_momentum(5).unwrap_or(0.0);
        let volume_ratio = self.calculate_volume_ratio().unwrap_or(1.0);

        let indicators = MomentumIndicators {
            rsi,
            macd,
            price_momentum,
            volume_ratio,
            overall_signal: MomentumSignal::Neutral, // Will be calculated next
        };

        let overall_signal = self.get_overall_momentum_signal(&indicators);

        Some(MomentumIndicators {
            overall_signal,
            ..indicators
        })
    }

    /// Generates momentum alerts based on current indicators
    pub fn generate_alerts(&self, indicators: &MomentumIndicators) -> Vec<String> {
        let mut alerts = Vec::new();

        // RSI alerts
        if indicators.rsi > 75.0 {
            alerts.push("⚠️ RSI severely overbought - potential reversal incoming".to_string());
        } else if indicators.rsi < 25.0 {
            alerts.push("🚀 RSI severely oversold - potential bounce opportunity".to_string());
        }

        // MACD alerts
        if indicators.macd.histogram > 100.0 {
            alerts.push("📈 Strong MACD bullish momentum detected".to_string());
        } else if indicators.macd.histogram < -100.0 {
            alerts.push("📉 Strong MACD bearish momentum detected".to_string());
        }

        // Price momentum alerts
        if indicators.price_momentum > 2.0 {
            alerts.push("🔥 Strong positive price momentum - trend acceleration".to_string());
        } else if indicators.price_momentum < -2.0 {
            alerts.push("❄️ Strong negative price momentum - trend deceleration".to_string());
        }

        // Volume alerts
        if indicators.volume_ratio > 2.0 {
            alerts.push("📊 Exceptional volume detected - significant market interest".to_string());
        }

        if alerts.is_empty() {
            alerts.push("✅ No momentum alerts - markets stable".to_string());
        }

        alerts
    }

    /// Gets current price
    pub fn get_current_price(&self) -> Option<f64> {
        self.price_history.back().copied()
    }

    /// Gets price change from previous data point
    pub fn get_price_change(&self) -> Option<(f64, f64)> {
        if self.price_history.len() < 2 {
            return None;
        }

        let current = *self.price_history.back()?;
        let previous = self.price_history[self.price_history.len() - 2];
        let change = current - previous;
        let change_percent = (change / previous) * 100.0;

        Some((change, change_percent))
    }

    /// Gets the last N price data points
    pub fn get_recent_prices(&self, count: usize) -> Vec<f64> {
        self.price_history
            .iter()
            .rev()
            .take(count)
            .rev()
            .cloned()
            .collect()
    }

    /// Gets the last N volume data points
    pub fn get_recent_volumes(&self) -> Vec<f64> {
        self.volume_history.iter().cloned().collect()
    }

    /// Prints formatted momentum report
    pub fn print_momentum_report(&self) {
        if let Some(indicators) = self.calculate_all_indicators() {
            println!("=== Bitcoin 5M Momentum Report ===");

            if let Some(price) = self.get_current_price() {
                println!("Current Price: ${:.2}", price);
            }

            if let Some((change, change_percent)) = self.get_price_change() {
                let sign = if change >= 0.0 { "+" } else { "" };
                println!(
                    "Price Change: {}{:.2} ({}{:.2}%)",
                    sign, change, sign, change_percent
                );
            }

            println!("\nTechnical Indicators:");
            println!(
                "RSI (14): {:.1} - {:?}",
                indicators.rsi,
                self.get_rsi_signal(indicators.rsi)
            );
            println!(
                "MACD Histogram: {:.4} - {:?}",
                indicators.macd.histogram,
                self.get_macd_signal(&indicators.macd)
            );
            println!(
                "Price Momentum: {:.2}% - {:?}",
                indicators.price_momentum,
                self.get_price_momentum_signal(indicators.price_momentum)
            );
            println!("Volume Ratio: {:.2}x", indicators.volume_ratio);
            println!("\nOverall Signal: {:?}", indicators.overall_signal);

            println!("\nAlerts:");
            for alert in self.generate_alerts(&indicators) {
                println!("  {}", alert);
            }
        } else {
            println!("Insufficient data for momentum analysis");
        }
    }
}

/// Generates simulated Bitcoin price data for testing
pub fn generate_sample_data(tracker: &mut BitcoinMomentumTracker, num_points: usize) {
    let mut base_price = 65000.0 + (rand::random::<f64>() * 5000.0);

    for i in 0..num_points {
        // Simulate realistic Bitcoin price movement
        let volatility = 0.003; // 0.3% average movement per 5min
        let trend = (i as f64 * 0.1).sin() * 0.001; // Slight trending component
        let random_walk = (rand::random::<f64>() - 0.5) * volatility;

        base_price *= 1.0 + trend + random_walk;

        // Generate volume (20M to 35M USD typical range)
        let volume = 20_000_000.0 + (rand::random::<f64>() * 15_000_000.0);

        tracker.add_data_point(base_price, volume);

        // Simulate 5-minute intervals
        std::thread::sleep(std::time::Duration::from_millis(10)); // Fast simulation
    }
}

// Helper module for random number generation
mod rand {
    use std::cell::RefCell;
    use std::time::{SystemTime, UNIX_EPOCH};

    thread_local! {
        static RNG_STATE: RefCell<u64> = RefCell::new(
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
        );
    }

    pub fn random<T>() -> T
    where
        T: From<f64>,
    {
        RNG_STATE.with(|state| {
            let mut rng = state.borrow_mut();
            *rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
            let normalized = (*rng as f64) / (u64::MAX as f64);
            T::from(normalized)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_creation() {
        let tracker = BitcoinMomentumTracker::new(100);
        assert_eq!(tracker.max_history, 100);
        assert_eq!(tracker.price_history.len(), 0);
    }

    #[test]
    fn test_add_data_point() {
        let mut tracker = BitcoinMomentumTracker::new(10);
        tracker.add_data_point(65000.0, 25000000.0);

        assert_eq!(tracker.price_history.len(), 1);
        assert_eq!(tracker.volume_history.len(), 1);
        assert_eq!(*tracker.price_history.front().unwrap(), 65000.0);
    }

    #[test]
    fn test_rsi_calculation() {
        let mut tracker = BitcoinMomentumTracker::new(100);

        // Add insufficient data
        for i in 0..10 {
            tracker.add_data_point(65000.0 + (i as f64 * 100.0), 25000000.0);
        }
        assert!(tracker.calculate_rsi(14).is_none());

        // Add sufficient data
        for i in 10..30 {
            tracker.add_data_point(65000.0 + (i as f64 * 100.0), 25000000.0);
        }
        let rsi = tracker.calculate_rsi(14);
        assert!(rsi.is_some());
        assert!(rsi.unwrap() >= 0.0 && rsi.unwrap() <= 100.0);
    }

    #[test]
    fn test_momentum_signals() {
        let tracker = BitcoinMomentumTracker::new(100);

        assert_eq!(tracker.get_rsi_signal(80.0), MomentumSignal::Bearish);
        assert_eq!(tracker.get_rsi_signal(20.0), MomentumSignal::Bullish);
        assert_eq!(tracker.get_rsi_signal(50.0), MomentumSignal::Neutral);
    }

    #[test]
    fn test_price_momentum() {
        let mut tracker = BitcoinMomentumTracker::new(100);

        // Add data with clear upward momentum
        for i in 0..10 {
            tracker.add_data_point(65000.0 + (i as f64 * 500.0), 25000000.0);
        }

        let momentum = tracker.calculate_price_momentum(5);
        assert!(momentum.is_some());
        assert!(momentum.unwrap() > 0.0); // Should be positive momentum
    }

    #[test]
    fn test_max_history_limit() {
        let mut tracker = BitcoinMomentumTracker::new(5);

        // Add more data points than limit
        for i in 0..10 {
            tracker.add_data_point(65000.0 + (i as f64), 25000000.0);
        }

        assert_eq!(tracker.price_history.len(), 5);
        assert_eq!(*tracker.price_history.front().unwrap(), 65005.0); // Should keep last 5
    }
}

// Example usage and main function
pub async fn run_momentum_tracker() -> Result<(), anyhow::Error> {
    println!("Bitcoin 5-Minute Momentum Tracker");
    println!("=================================");

    let mut tracker = BitcoinMomentumTracker::new(100);

    // Generate sample data
    println!("Generating sample 5-minute Bitcoin data...");
    //generate_sample_data(&mut tracker, 50);
    let candle_data = Arc::new(HttpCandleData {
        client: reqwest::Client::new(),
        symbol: String::from("BTCUSDT"),
    });
    let res: Result<Vec<Candle>, anyhow::Error> = candle_data
        .get_bitget_candles(String::from("15m"), String::from("100"))
        .await;

    let candle_data = res.unwrap_or_else(|_| Vec::new());
    if candle_data.len() == 0 {
        return Ok(());
    }
    info!("candle_data: {:?}", candle_data);

    //Iterate the candle data and load the tracker with the results

    // Calculate and display momentum
    tracker.print_momentum_report();

    println!("\n--- Simulating real-time updates ---");

    // Simulate a few real-time updates
    for i in 0..5 {
        println!("\nUpdate {}:", i + 1);

        // Simulate new price data
        let last_price = tracker.get_current_price().unwrap_or(65000.0);
        let volatility = 0.005; // Higher volatility for demo
        let new_price = last_price * (1.0 + (rand::random::<f64>() - 0.5) * volatility);
        let new_volume = 20_000_000.0 + (rand::random::<f64>() * 15_000_000.0);

        tracker.add_data_point(new_price, new_volume);
        tracker.print_momentum_report();

        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    Ok(())
}

// Additional utility functions

impl MomentumIndicators {
    /// Formats the momentum indicators for display
    pub fn format_report(&self) -> String {
        format!(
            "RSI: {:.1} | MACD: {:.4} | Momentum: {:.2}% | Volume: {:.2}x | Signal: {:?}",
            self.rsi,
            self.macd.histogram,
            self.price_momentum,
            self.volume_ratio,
            self.overall_signal
        )
    }

    /// Checks if momentum is strongly bullish
    pub fn is_strong_bullish(&self) -> bool {
        matches!(self.overall_signal, MomentumSignal::Bullish)
            && self.price_momentum > 1.5
            && self.volume_ratio > 1.2
    }

    /// Checks if momentum is strongly bearish
    pub fn is_strong_bearish(&self) -> bool {
        matches!(self.overall_signal, MomentumSignal::Bearish)
            && self.price_momentum < -1.5
            && self.volume_ratio > 1.2
    }
}

impl PriceData {
    pub fn new(price: f64, volume: f64) -> Self {
        Self {
            price,
            volume,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }
}

// WebSocket integration example (commented out - would require tokio and websocket crates)
/*
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{StreamExt, SinkExt};

pub async fn start_live_tracking() -> Result<(), Box<dyn std::error::Error>> {
    let mut tracker = BitcoinMomentumTracker::new(288); // 24 hours of 5-min data

    // Connect to Binance WebSocket
    let url = "wss://stream.binance.com:9443/ws/btcusdt@kline_5m";
    let (ws_stream, _) = connect_async(url).await?;
    let (mut write, mut read) = ws_stream.split();

    while let Some(message) = read.next().await {
        match message? {
            Message::Text(data) => {
                // Parse Binance kline data and update tracker
                // This would require serde_json for JSON parsing
                println!("Received: {}", data);
            }
            _ => {}
        }
    }

    Ok(())
}
*/
