use anyhow::{Ok, Result};
use chrono::Utc;
use log::{info, warn};
use redis::AsyncCommands;
use uuid::Uuid;

use crate::{
    bot::{Bot, ClosedPosition, OpenPosition, Position, Zones},
    config::Config,
    exchange::{Exchange, OrderSide},
    helper::{
        CLOSED_POSITIONS, Helper, SCALPER_CLOSED_POSITIONS, TRADIN_SCALPER_BOT_POSITION,
        TRADING_SCALPER_BOT_ACTIVE,
    },
};

pub struct ScalperBot {
    pub scalp_open_pos: OpenPosition,

    pub scalp_pos: Position,

    pub zones: Zones,

    // a *mutable* reference to the redis connection
    redis_conn: redis::aio::MultiplexedConnection,
}

impl ScalperBot {
    pub async fn new(mut conn: redis::aio::MultiplexedConnection) -> Result<Self> {
        let zones: Zones = Bot::load_zones(&mut conn)
            .await
            .unwrap_or_else(|_| Zones::default());

        let open_pos = Self::load_scalper_open_position(&mut conn)
            .await
            .unwrap_or_else(|_| Self::default_scalper_open_position());
        warn!("open_pos -> {:?}", open_pos);

        Ok(Self {
            scalp_pos: open_pos.pos,
            zones,
            redis_conn: conn,
            scalp_open_pos: open_pos,
        })
    }

    async fn load_scalper_open_position(
        conn: &mut redis::aio::MultiplexedConnection,
    ) -> Result<OpenPosition> {
        let key = TRADING_SCALPER_BOT_ACTIVE;

        let open_pos: String = conn.get(key).await?;

        Ok(serde_json::from_str(&open_pos)?)
    }

    fn default_scalper_open_position() -> OpenPosition {
        OpenPosition {
            id: Uuid::nil(),
            pos: Position::Flat,
            entry_price: 0.00,
            entry_time: Utc::now(),
            position_size: 0.015,
            sl: Some(0.00),
            margin: Some(50.00),
            quantity: Some(0.015),
            risk_pct: Some(0.05),
            leverage: Some(35.00),
        }
    }

    async fn store_position(&mut self, pos: Position, open_pos: OpenPosition) -> Result<()> {
        let _: () = self
            .redis_conn
            .set(TRADIN_SCALPER_BOT_POSITION, pos.as_str())
            .await?;

        let scalper_key = TRADING_SCALPER_BOT_ACTIVE;

        let _: () = self.redis_conn.set(scalper_key, open_pos.as_str()).await?;

        Ok(())
    }

    /// Store *one* closed position in the list named `"closed_positions" & "scalper_closed_positions"`.
    pub async fn store_closed_position(
        conn: &mut redis::aio::MultiplexedConnection,
        pos: &ClosedPosition,
    ) -> Result<()> {
        //use the same as the ranger and other bots
        let key = CLOSED_POSITIONS;

        //Now this one is for the scalper_closed_positions so we can track the difference in performance
        let scalper_key = SCALPER_CLOSED_POSITIONS;
        let json = serde_json::to_string(pos)?;

        // LPUSH pushes to the **left** of the list – newest element first
        let _: () = conn.lpush(key, json.clone()).await?;

        // RPUSH pushes to the **right** of the list - oldest element first
        let _: () = conn.rpush(scalper_key, json.clone()).await?;

        // OPTIONAL: keep only the last N trades (e.g. 10 000)
        // conn.ltrim(key, 0, 9999).await?;

        //Delete the open_position
        let _: usize = conn.del(TRADING_SCALPER_BOT_ACTIVE).await?;

        Ok(())
    }

    pub fn prepare_open_position(
        &mut self,
        pos: Position,
        entry_price: f64,
        position_size: f64,
        margin: f64,
        leverage: f64,
        risk_pct: f64,
    ) -> OpenPosition {
        let sl = Helper::stop_loss_price(entry_price, margin, leverage, risk_pct, pos);
        let qty = Helper::contract_amount(entry_price, margin, leverage);
        OpenPosition {
            id: Uuid::new_v4(),
            pos: pos,
            entry_price: entry_price,
            position_size, //does the same thing as quantity :(
            entry_time: Utc::now(),
            sl: Some(sl),
            margin: Some(margin),
            quantity: Some(qty),
            leverage: Some(leverage),
            risk_pct: Some(risk_pct),
        }
    }

    async fn close_long_position(&mut self, price: f64, config: &mut Config) {
        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.scalp_open_pos.margin.unwrap_or(config.margin),
            self.scalp_open_pos.entry_price,
            self.scalp_pos,
            self.scalp_open_pos.position_size,
            price,
        );
        let closed_pos = ClosedPosition {
            id: self.scalp_open_pos.id,
            entry_price: self.scalp_open_pos.entry_price,
            exit_price: price,
            exit_time: Utc::now(),
            position: Some(Position::Long),
            side: None,
            entry_time: self.scalp_open_pos.entry_time,
            pnl: Helper::compute_pnl(
                self.scalp_pos,
                self.scalp_open_pos.entry_price,
                self.scalp_open_pos.position_size,
                price,
            ),
            quantity: Some(self.scalp_open_pos.position_size),
            sl: self.scalp_open_pos.sl,
            roi: Some(roi),
            leverage: self.scalp_open_pos.leverage,
            margin: self.scalp_open_pos.margin,
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;
    }

    async fn close_short_position(&mut self, price: f64, config: &mut Config) {
        let pnl = Helper::compute_pnl(
            self.scalp_open_pos.pos,
            self.scalp_open_pos.entry_price,
            self.scalp_open_pos.position_size,
            price,
        );
        let roi = Helper::calc_roi(
            &mut Helper::from_config(),
            self.scalp_open_pos.margin.unwrap_or(config.margin),
            self.scalp_open_pos.entry_price,
            self.scalp_open_pos.pos,
            self.scalp_open_pos.position_size,
            price,
        );
        let closed_pos = ClosedPosition {
            id: self.scalp_open_pos.id,
            entry_price: self.scalp_open_pos.entry_price,
            exit_price: price,
            exit_time: Utc::now(),
            position: Some(Position::Short),
            side: None,
            entry_time: self.scalp_open_pos.entry_time,
            pnl,
            quantity: Some(self.scalp_open_pos.position_size),
            sl: self.scalp_open_pos.sl,
            roi: Some(roi),
            leverage: self.scalp_open_pos.leverage,
            margin: self.scalp_open_pos.margin,
        };
        let _ = Self::store_closed_position(&mut self.redis_conn, &closed_pos).await;
    }

    pub async fn take_profit_on_long(
        &mut self,
        price: f64,
        size: f64,
        config: &mut Config,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        info!("Scalper Taking profit on LONG at {:.2}", price);

        let exec_price = exchange.place_market_order(OrderSide::Sell, size).await?;

        info!("Scalper Closed LONG at {:.2}", exec_price);

        Self::close_long_position(self, price, config).await;

        self.scalp_pos = Position::Flat;

        Ok(())
    }

    pub async fn take_profit_on_short(
        &mut self,
        price: f64,
        size: f64,
        config: &mut Config,
        exchange: &dyn Exchange,
    ) -> Result<()> {
        info!("Scalper Covering SHORT at {:.2}", price);

        let exec_price = exchange.place_market_order(OrderSide::Buy, size).await?;

        info!("Scalper Covered SHORT at {:.2}", exec_price);

        Self::close_short_position(self, price, config).await;

        self.scalp_pos = Position::Flat;

        Ok(())
    }

    pub async fn run_scalper_bot(
        &mut self,
        price: f64,
        exchange: &dyn Exchange,
        config: &mut Config,
    ) -> Result<()> {
        warn!("Scalper State = {:?}", self.scalp_pos);
        let default_size = Helper::contract_amount(price, config.margin, config.leverage);

        match self.scalp_pos {
            Position::Flat => {
                if self.zones.long_zones.iter().any(|z| z.contains(price)) {
                    info!("Scalper is Entering LONG at {:.2}", price);

                    let exec_price = exchange
                        .place_market_order(OrderSide::Buy, default_size)
                        .await?;
                    info!("Scalper Long executed at {:.2}", exec_price);

                    self.scalp_pos = Position::Long;

                    self.scalp_open_pos = Self::prepare_open_position(
                        self,
                        self.scalp_pos,
                        price,
                        default_size,
                        config.margin,
                        config.leverage,
                        config.risk_pct,
                    );
                    self.store_position(self.scalp_pos, self.scalp_open_pos)
                        .await?;
                } else if self.zones.short_zones.iter().any(|z| z.contains(price)) {
                    info!("Scalper is Entering SHORT at {:.2}", price);

                    let exec_price = exchange
                        .place_market_order(OrderSide::Sell, default_size)
                        .await?;

                    info!("Scalper Short executed at {:.2}", exec_price);

                    self.scalp_pos = Position::Short;

                    self.scalp_open_pos = Self::prepare_open_position(
                        self,
                        Position::Short,
                        price,
                        default_size,
                        config.margin,
                        config.leverage,
                        config.risk_pct,
                    );
                    self.store_position(self.scalp_pos, self.scalp_open_pos)
                        .await?;
                }
            }

            Position::Long => {
                //Trigger SL if it's met
                let in_sl = Helper::stop_loss_price(
                    self.scalp_open_pos.entry_price,
                    config.margin,
                    config.leverage,
                    config.risk_pct,
                    Position::Long,
                );
                let ssl_hit = Helper::ssl_hit(
                    price,
                    self.scalp_pos,
                    self.scalp_open_pos.sl.unwrap_or(in_sl),
                );

                if ssl_hit {
                    Self::close_long_position(self, price, config).await;

                    warn!(
                        "SL for Scalper Long Position entered at {:2}, with SL triggered at {:2}",
                        self.scalp_open_pos.entry_price, price
                    );

                    self.scalp_pos = Position::Flat;
                }

                let config_diff = config.scalp_price_difference;
                let min_config_diff = config_diff - 100.00;
                let diff = Helper::calc_price_difference(
                    self.scalp_open_pos.entry_price,
                    price,
                    Position::Long,
                );
                info!(
                    "SCALPER diff >= config_diff {:2.2} >= {:2.2}",
                    diff, config_diff
                );

                if diff >= config_diff || diff >= min_config_diff {
                    //Take your profits and get out!
                    Self::take_profit_on_long(self, price, default_size, config, exchange).await?;
                }
            }

            Position::Short => {
                //Trigger SL if it's met
                let in_sl = Helper::stop_loss_price(
                    self.scalp_open_pos.entry_price,
                    config.margin,
                    config.leverage,
                    config.risk_pct,
                    Position::Long,
                );
                let ssl_hit = Helper::ssl_hit(
                    price,
                    self.scalp_pos,
                    self.scalp_open_pos.sl.unwrap_or(in_sl),
                );

                if ssl_hit {
                    Self::close_short_position(self, price, config).await;

                    warn!(
                        "SL for Scalper Short Position entered at {:2}, with SL triggered at {:2}",
                        self.scalp_open_pos.entry_price, price
                    );

                    self.scalp_pos = Position::Flat;
                }

                //Operation scalp, if set
                let config_diff = config.scalp_price_difference;
                let min_config_diff = config_diff - 100.00;
                let diff = Helper::calc_price_difference(
                    self.scalp_open_pos.entry_price,
                    price,
                    Position::Short,
                );
                info!(
                    "SCALPER diff >= config_diff {:2.2} >= {:2.2}",
                    diff, config_diff
                );
                if diff >= config_diff || diff >= min_config_diff {
                    //Take your profits and get out!
                    Self::take_profit_on_short(self, price, default_size, config, exchange).await?;
                }
            }
        }
        Ok(())
    }
}
