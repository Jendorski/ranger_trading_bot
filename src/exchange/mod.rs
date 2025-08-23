use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, Copy)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[async_trait]
pub trait Exchange: Send + Sync {
    /// Return the latest spot price for the configured symbol.
    async fn get_current_price(&self) -> Result<f64>;

    /// Place a market order.  
    /// `side` is BUY for long, SELL for short/cover.  
    /// Returns the executed price (for logging).
    async fn place_market_order(&self, side: OrderSide, amount: f64) -> Result<f64>;
}
