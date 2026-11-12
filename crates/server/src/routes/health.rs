//! `GET /api/v1/health`：伺服器版本與資料庫連線狀態，不需要登入。
//!
//! - 資料庫正常 → `200`，`status: "ok"`
//! - 資料庫異常 → `503`，`status: "error"`（伺服器本身仍繼續運作）
//!
//! 回應型別定義在 [`hammer_shared::health`]，桌面程式共用。

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use hammer_core::db::sqlx;
use hammer_shared::health::{HealthResponse, HealthStatus as Status};

use crate::{AppState, SERVER_VERSION};

/// 資料庫檢查的逾時時間。
const DB_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let check = tokio::time::timeout(
        DB_CHECK_TIMEOUT,
        sqlx::query("SELECT 1").execute(&state.pool),
    )
    .await;

    let database = match check {
        Ok(Ok(_)) => Status::Ok,
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "health：資料庫連線異常");
            Status::Error
        }
        Err(_) => {
            tracing::warn!(timeout = ?DB_CHECK_TIMEOUT, "health：資料庫檢查逾時");
            Status::Error
        }
    };

    let http_status = match database {
        Status::Ok => StatusCode::OK,
        Status::Error => StatusCode::SERVICE_UNAVAILABLE,
    };
    let body = HealthResponse {
        status: database,
        version: SERVER_VERSION.to_owned(),
        database,
    };
    (http_status, Json(body))
}
