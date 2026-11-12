//! Hammer ERP API Server（axum）。
//!
//! 啟動流程（[`run`]）：載入設定 → 初始化 log → 建立連線池 → 套用 migrations
//! → 啟動 HTTP → 收到 Ctrl+C / SIGTERM 時等進行中的請求完成再關閉。
//!
//! 相依方向：`server → core → db`、`server → shared`（`docs/architecture.md` §4）。

pub mod config;
pub mod error;
pub mod middleware;
pub mod routes;
pub mod shutdown;
pub mod state;
pub mod telemetry;

use anyhow::Context;
use hammer_core::db::{self, PoolConfig};

pub use config::Config;
pub use error::AppError;
pub use routes::router;
pub use state::AppState;

/// 伺服器版本（`Cargo.toml` 的 `version`）。
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 依環境變數與設定檔啟動伺服器，直到收到關閉訊號。
pub async fn run() -> anyhow::Result<()> {
    // 開發時從根目錄 .env 讀取 DATABASE_URL 等設定；檔案不存在時略過
    let _ = dotenvy::dotenv();

    let config = Config::load().context("載入設定失敗")?;
    telemetry::init(&config).context("初始化 log 失敗")?;
    tracing::info!(version = SERVER_VERSION, listen_addr = %config.listen_addr, "啟動 API Server");

    std::fs::create_dir_all(&config.attachments_dir)
        .with_context(|| format!("無法建立附件目錄：{}", config.attachments_dir.display()))?;

    let pool = db::connect(&PoolConfig::new(config.database_url.clone()))
        .await
        .context("連線資料庫失敗")?;
    db::run_migrations(&pool)
        .await
        .context("套用 migrations 失敗")?;
    tracing::info!("資料庫 migrations 已套用");

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("無法監聽 {}", config.listen_addr))?;
    tracing::info!(addr = %listener.local_addr()?, "開始接受連線");

    let state = AppState::new(pool.clone(), config);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown::signal())
        .await
        .context("HTTP 伺服器錯誤")?;

    pool.close().await;
    tracing::info!("伺服器已關閉");
    Ok(())
}
