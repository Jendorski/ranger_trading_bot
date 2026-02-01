//1st February 2026. Not the time to use this.
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize)]
struct PredictionRequest {
    text: String,
}

#[derive(Debug, Deserialize)]
pub struct PredictionResponse {
    pub sentiment: i64, // 0: Bearish, 1: Neutral, 2: Bullish
    pub label: String,
    pub confidence: f64,
}

pub struct SentimentClient {
    client: Client,
    endpoint: String,
}

impl SentimentClient {
    pub fn new(endpoint: Option<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.unwrap_or_else(|| "http://localhost:8000/predict".to_string()),
        }
    }

    /// Fetches the sentiment for a given piece of text (e.g. video transcript)
    pub async fn get_sentiment(&self, text: &str) -> Result<PredictionResponse> {
        let payload = PredictionRequest {
            text: text.to_string(),
        };

        let response = self
            .client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await?
            .json::<PredictionResponse>()
            .await?;

        Ok(response)
    }

    /// Convenience method to check if market is 'safe' for bullish trades
    pub async fn is_bullish(&self, text: &str) -> bool {
        match self.get_sentiment(text).await {
            Ok(res) => res.sentiment == 2,
            Err(_) => false,
        }
    }
}
