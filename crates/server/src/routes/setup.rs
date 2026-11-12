//! 初次啟動建立第一個管理者（docs/architecture.md §11）。
//!
//! | 端點 | 認證 | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/setup/status` | 不需要 | `200` [`SetupStatus`] |
//! | `POST /api/v1/setup/admin` | 不需要 | `200` [`LoginResponse`] |
//!
//! `POST /setup/admin` 錯誤：
//!
//! - 已有任何帳號 → `409 CONFLICT`
//! - 帳號、顯示名稱空白或密碼不足 10 個字元 → `422 VALIDATION`

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use hammer_core::setup::{self, InitialAdmin};
use hammer_shared::auth::LoginResponse;
use hammer_shared::setup::{CreateAdminRequest, SetupStatus};
use time::OffsetDateTime;

use super::auth::{current_user, request_client_info};
use crate::{AppError, AppState};

/// `GET /api/v1/setup/status`
pub async fn status(State(state): State<AppState>) -> Result<Json<SetupStatus>, AppError> {
    let needs_setup = setup::needs_setup(&state.pool).await?;
    Ok(Json(SetupStatus { needs_setup }))
}

/// `POST /api/v1/setup/admin`
pub async fn create_admin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateAdminRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    let info = request_client_info(&headers);
    let outcome = setup::create_initial_admin(
        &state.pool,
        &InitialAdmin {
            username: &req.username,
            display_name: &req.display_name,
            password: &req.password,
        },
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(LoginResponse {
        token: outcome.token.into_string(),
        user: current_user(&outcome.user),
    }))
}
