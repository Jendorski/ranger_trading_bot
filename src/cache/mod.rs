use redis::aio::MultiplexedConnection;
use redis::{Client, RedisError};

pub struct RedisClient {
    conn: MultiplexedConnection,
}

impl RedisClient {
    pub async fn connect(url: &str) -> Result<Self, RedisError> {
        let client = Client::open(url)?;
        let mut delay = std::time::Duration::from_secs(1);
        let max_delay = std::time::Duration::from_secs(32);
        let mut retries = 0;
        let max_retries = 10;

        loop {
            match client.get_multiplexed_async_connection().await {
                Ok(conn) => {
                    log::info!("Successfully connected to Redis at {}", url);
                    return Ok(Self { conn });
                }
                Err(e) => {
                    retries += 1;
                    if retries > max_retries {
                        log::error!(
                            "Failed to connect to Redis after {} attempts: {}",
                            max_retries,
                            e
                        );
                        return Err(e);
                    }
                    log::warn!(
                        "Redis connection failed (attempt {}/{}): {}. Retrying in {:?}...",
                        retries,
                        max_retries,
                        e,
                        delay
                    );
                    tokio::time::sleep(delay).await;
                    delay = std::cmp::min(delay * 2, max_delay);
                }
            }
        }
    }

    #[inline]
    pub fn get_multiplexed_connection(&self) -> MultiplexedConnection {
        self.conn.clone()
    }
}
