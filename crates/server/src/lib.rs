//! Hammer ERP API Server（axum）。
//!
//! 啟動流程（[`run`]）：載入設定 → 初始化 log → 建立連線池 → 套用 migrations
//! → 啟動附件清理排程（[`spawn_attachment_cleanup`]）→ 啟動 HTTP
//! → 收到 Ctrl+C / SIGTERM 時等進行中的請求完成再關閉。
//!
//! 相依方向：`server → core → db`、`server → shared`（`docs/architecture.md` §4）。

pub mod auth;
pub mod config;
pub mod error;
pub mod middleware;
pub mod routes;
pub mod shutdown;
pub mod state;
pub mod telemetry;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use hammer_core::attachments::{AttachmentStore, cleanup_unbound};
use hammer_core::db::sqlx::PgPool;
use hammer_core::db::{self, PoolConfig};

pub use auth::{AuthUser, AuthUserAllowPasswordChange};
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
    let cleanup = spawn_attachment_cleanup(pool.clone(), state.attachments.clone());
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown::signal())
        .await
        .context("HTTP 伺服器錯誤")?;

    cleanup.abort();
    pool.close().await;
    tracing::info!("伺服器已關閉");
    Ok(())
}

/// 附件清理的間隔。
pub const ATTACHMENT_CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 背景執行未綁定附件的清理（[`cleanup_unbound`]）：啟動時執行一次，之後每
/// [`ATTACHMENT_CLEANUP_INTERVAL`] 執行一次。失敗只寫 log，下次照常執行。
pub fn spawn_attachment_cleanup(
    pool: PgPool,
    store: Arc<AttachmentStore>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(ATTACHMENT_CLEANUP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now = time::OffsetDateTime::now_utc();
            if let Err(err) = cleanup_unbound(&pool, &store, now).await {
                tracing::error!(error = %err, "清理未綁定的附件失敗");
            }
        }
    })
}
