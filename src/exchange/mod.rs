use anyhow::Ok;
use anyhow::Result;
use async_trait::async_trait;
use log::info;

use crate::exchange::bitget::Prices;

pub mod bitget;

#[derive(Debug, Clone, Copy)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[async_trait]
pub trait Exchange: Send + Sync {
    /// Return the latest spot price for the configured symbol.
    async fn get_bitget_price(&self) -> Result<f64>;

    /// Return the latest spot price for the configured symbol.
    async fn get_current_price(&self) -> Result<f64>;

    /// Place a market order.  
    /// `side` is BUY for long, SELL for short/cover.  
    /// Returns the executed price (for logging).
    async fn place_market_order(&self, side: OrderSide, amount: f64) -> Result<f64>;
}

/// Simple HTTP‑based mock of the `Exchange` trait – replace with your real SDK.
///
/// In this example we hit a public ticker endpoint (e.g. Binance).
pub struct HttpExchange {
    pub client: reqwest::Client,
    pub(crate) symbol: String,
}

#[async_trait::async_trait]
impl Exchange for HttpExchange {
    async fn get_bitget_price(&self) -> Result<f64, anyhow::Error> {
        //Bitget Futures Price API: https://api.bitget.com/api/v2/mix/market/symbol-price?productType=usdt-futures&symbol=BTCUSDT
        let bitget = self
            .client
            .get(format!("https://api.bitget.com/api/v2/mix/market/symbol-price?productType=usdt-futures&symbol={}", self.symbol))
            .send()
            .await?;
        // info!("bitget -> {:?}", bitget);

        let bitget_data = bitget.text().await?;

        let prices: Result<Prices, String> =
            bitget::get_prices(&bitget_data).ok_or_else(|| 0.00.to_string()); //"Failed to parse price data".into()

        let exchange_price = prices.unwrap_or(Prices {
            price: 0.00,
            index_price: 0.00,
            mark_price: 0.00,
        }); //.unwrap();

        Ok(exchange_price.mark_price)
    }

    async fn get_current_price(&self) -> Result<f64, anyhow::Error> {
        //let current_exchange = "bitget";

        // Example: Binance spot ticker
        // let resp = self
        //     .client
        //     .get(format!(
        //         "https://api.binance.com/api/v3/ticker/price?symbol={}",
        //         self.symbol
        //     ))
        //     .send()
        //     .await?
        //     .json::<serde_json::Value>()
        //     .await?;
        //let price: f64 = resp["price"].as_str().unwrap_or("0").parse()?;

        let bitget_price = Self::get_bitget_price(self).await?;

        return Ok(bitget_price);
    }

    async fn place_market_order(&self, side: OrderSide, amount: f64) -> Result<f64, anyhow::Error> {
        // For demo purposes we just log and pretend the order filled at current price.
        let price = self.get_current_price().await?;
        info!(
            "Mock market {:?} for {:.6} {} at {price:.2}",
            side, amount, self.symbol
        );
        Ok(price)
    }
}
