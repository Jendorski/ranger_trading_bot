use redis::aio::MultiplexedConnection;
use redis::{Client, RedisError};

pub struct RedisClient {
    conn: MultiplexedConnection,
}

impl RedisClient {
    pub async fn connect(url: &str) -> Result<Self, RedisError> {
        let client = Client::open(url);
        let conn = client?.get_multiplexed_async_connection().await?;

        Ok(Self { conn })
    }

    #[inline]
    pub fn get_multiplexed_connection(&self) -> MultiplexedConnection {
        self.conn.clone()
    }
}
