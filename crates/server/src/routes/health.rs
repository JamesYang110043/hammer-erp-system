//! `GET /api/v1/health`：伺服器版本與資料庫連線狀態，不需要登入。
//!
//! - 資料庫正常 → `200`，`status: "ok"`
//! - 資料庫異常 → `503`，`status: "error"`（伺服器本身仍繼續運作）

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use hammer_core::db::sqlx;
use serde::{Deserialize, Serialize};

use crate::{AppState, SERVER_VERSION};

/// 資料庫檢查的逾時時間。
const DB_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    /// 整體狀態。
    pub status: Status,
    /// 伺服器版本。
    pub version: String,
    /// 資料庫連線狀態。
    pub database: Status,
}

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
