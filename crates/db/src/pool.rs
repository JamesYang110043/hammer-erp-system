//! 連線池建立。

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::DbError;

/// 連線池設定。由外部（例如 server 讀取環境變數）傳入。
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// 連線字串，例如 `postgres://user:pass@127.0.0.1:5432/hammer_erp`。
    pub url: String,
    /// 最大連線數。
    pub max_connections: u32,
    /// 從連線池取得連線的逾時時間。
    pub acquire_timeout: Duration,
}

impl PoolConfig {
    /// 預設最大連線數。5 人以內使用，10 條已足夠。
    pub const DEFAULT_MAX_CONNECTIONS: u32 = 10;
    /// 預設取得連線逾時。
    pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

    /// 以預設的連線數與逾時建立設定。
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: Self::DEFAULT_MAX_CONNECTIONS,
            acquire_timeout: Self::DEFAULT_ACQUIRE_TIMEOUT,
        }
    }
}

/// 建立連線池，並立即建立一條連線確認資料庫可用。
pub async fn connect(config: &PoolConfig) -> Result<PgPool, DbError> {
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(config.acquire_timeout)
        .connect(&config.url)
        .await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_uses_defaults() {
        let config = PoolConfig::new("postgres://localhost/db");
        assert_eq!(config.url, "postgres://localhost/db");
        assert_eq!(config.max_connections, PoolConfig::DEFAULT_MAX_CONNECTIONS);
        assert_eq!(config.acquire_timeout, PoolConfig::DEFAULT_ACQUIRE_TIMEOUT);
    }
}
