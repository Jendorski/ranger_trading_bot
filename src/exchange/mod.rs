use anyhow::Ok;
use anyhow::Result;
use async_trait::async_trait;
use log::info;

use crate::bot::OpenPosition;
use crate::exchange::bitget::fees::VipFeeRate;
use crate::exchange::bitget::CandleData;
use crate::exchange::bitget::FuturesCall;
use crate::exchange::bitget::HttpCandleData;
use crate::exchange::bitget::PlaceOrderData;
use crate::exchange::bitget::Prices;
use crate::exchange::bitunix::BitunixHttpClient;
use crate::exchange::bitunix::fees::BitunixFuturesFees;

pub mod bitget;
pub mod bitunix;

#[async_trait]
pub trait Exchange: Send + Sync {
    /// Return the latest spot price for the configured symbol.
    async fn get_bitget_price(&self) -> Result<f64>;

    /// Return the latest spot price for the configured symbol.
    async fn get_current_price(&self) -> Result<f64>;

    /// Place a market order.
    /// `side` is BUY for long, SELL for short/cover.
    /// Returns the executed price (for logging).
    async fn place_market_order(&self, open_position: &OpenPosition) -> Result<PlaceOrderData>;

    ///Used for executing taking profits and executing SL
    async fn modify_market_order(&self, open_position: &OpenPosition) -> Result<PlaceOrderData>;

    /// Return the latest funding rate as a f64.
    async fn get_funding_rate(&self) -> Result<f64>;
    #[allow(dead_code)]
    async fn get_fee_rates(&self) -> Result<VipFeeRate>;

    /// Fetch the exchange-assigned position ID for the currently open position.
    /// Only meaningful for Bitunix (which requires a positionId for TPSL/close).
    /// Default: always returns None (Bitget does not use positionId).
    async fn get_position_id(&self) -> Result<Option<String>> {
        Ok(None)
    }

    /// Register the initial TP/SL order on a newly opened position.
    /// Only meaningful for Bitunix (Bitget embeds TPSL in the order itself).
    /// Default: no-op.
    async fn place_initial_tpsl(
        &self,
        _position_id: &str,
        _tp_price: Option<f64>,
        _sl_price: Option<f64>,
    ) -> Result<()> {
        Ok(())
    }
}

/// Simple HTTP‑based mock of the `Exchange` trait – replace with your real SDK.
///
/// In this example we hit a public ticker endpoint (e.g. Binance).
pub struct HttpExchange {
    pub client: reqwest::Client,
    pub(crate) symbol: String,
    #[allow(dead_code)]
    pub redis_conn: redis::aio::MultiplexedConnection,
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

        let bitget_data = bitget.text().await?;

        let prices: Result<Prices, String> =
            bitget::get_prices(&bitget_data).ok_or_else(|| 1.11.to_string()); //"Failed to parse price data".into()

        let exchange_price = prices.unwrap_or(Prices {
            price: 1.11,
            index_price: 1.11,
            mark_price: 1.11,
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

    async fn place_market_order(
        &self,
        open_position: &OpenPosition,
    ) -> Result<PlaceOrderData, anyhow::Error> {
        let new_bitget_futures = <HttpCandleData as bitget::FuturesCall>::new();
        let execute_call = new_bitget_futures.new_futures_call(open_position).await?;
        Ok(execute_call)
    }

    async fn modify_market_order(
        &self,
        open_position: &OpenPosition,
    ) -> Result<PlaceOrderData, anyhow::Error> {
        let price = self.get_current_price().await?;
        info!(
            "Mock market {:?} for {:.6} {} at {price:.2}",
            open_position.pos, open_position.entry_price, self.symbol
        );
        let new_bitget_futures = <HttpCandleData as bitget::FuturesCall>::new();
        let execute_call = new_bitget_futures
            .modify_futures_order(open_position)
            .await?;
        Ok(execute_call)
    }

    async fn get_funding_rate(&self) -> Result<f64, anyhow::Error> {
        let bitget_data = <HttpCandleData as bitget::CandleData>::new();
        let funding_rates = bitget_data
            .get_history_funding_rate("1".to_string())
            .await?;
        if let Some(first) = funding_rates.first() {
            return Ok(first.funding_rate.parse::<f64>().unwrap_or(0.0));
        }
        Ok(0.0)
    }

    async fn get_fee_rates(&self) -> Result<VipFeeRate, anyhow::Error> {
        let conn = self.redis_conn.clone();
        let fees = bitget::fees::BitgetFuturesFees::new(conn);
        let bitget_data = fees.get_vip_fee_rates().await?;
        Ok(bitget_data.first().unwrap().clone())
    }
}

// ─── Bitunix exchange implementation ─────────────────────────────────────────

pub struct BitunixExchange {
    pub client: BitunixHttpClient,
    pub fees: BitunixFuturesFees,
}

impl BitunixExchange {
    pub fn new(config: &crate::config::Config) -> Self {
        Self {
            client: BitunixHttpClient::new(config),
            fees: BitunixFuturesFees::new(config.bitunix_maker_fee, config.bitunix_taker_fee),
        }
    }
}

#[async_trait::async_trait]
impl Exchange for BitunixExchange {
    async fn get_bitget_price(&self) -> Result<f64> {
        // Bitunix has no Bitget endpoint; delegate to get_current_price.
        self.get_current_price().await
    }

    async fn get_current_price(&self) -> Result<f64> {
        self.client.get_current_price().await
    }

    /// Place a market entry order.
    /// SL is embedded in the order body; TP/SL registration via `place_initial_tpsl`.
    async fn place_market_order(&self, open_position: &OpenPosition) -> Result<PlaceOrderData> {
        self.client.place_order(open_position).await
    }

    /// Close (or partially close) a position.
    /// Uses a reduce-only CLOSE-side market order so it handles both full and partial TP.
    async fn modify_market_order(&self, open_position: &OpenPosition) -> Result<PlaceOrderData> {
        let qty = open_position.position_size.to_string();
        self.client.close_partial(open_position, &qty).await
    }

    async fn get_funding_rate(&self) -> Result<f64> {
        self.client.get_funding_rate().await
    }

    async fn get_fee_rates(&self) -> Result<VipFeeRate> {
        Ok(self.fees.as_vip_fee_rate())
    }

    async fn get_position_id(&self) -> Result<Option<String>> {
        self.client.get_pending_position_id().await
    }

    async fn place_initial_tpsl(
        &self,
        position_id: &str,
        tp_price: Option<f64>,
        sl_price: Option<f64>,
    ) -> Result<()> {
        self.client
            .place_position_tpsl(position_id, tp_price, sl_price)
            .await
            .map(|_| ())
    }
}
