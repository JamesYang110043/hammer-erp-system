//! Application state：所有 handler 共用的資源。

use std::sync::Arc;

use hammer_core::db::sqlx::PgPool;

use crate::Config;

/// 所有 handler 共用的狀態。clone 成本低（內部是 `Arc` 與連線池 handle）。
///
/// 之後的 auth（session 驗證服務等）加在這裡。
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        Self {
            pool,
            config: Arc::new(config),
        }
    }
}
