//! 登入、登出、目前使用者、修改密碼（docs/architecture.md §6.2、§10）。
//!
//! | 端點 | 認證 | 成功回應 |
//! |---|---|---|
//! | `POST /api/v1/auth/login` | 不需要 | `200` [`LoginResponse`] |
//! | `POST /api/v1/auth/logout` | [`AuthUserAllowPasswordChange`] | `204` |
//! | `GET /api/v1/auth/me` | [`AuthUserAllowPasswordChange`] | `200` [`CurrentUser`] |
//! | `POST /api/v1/auth/change-password` | [`AuthUserAllowPasswordChange`] | `204` |
//!
//! 登入錯誤：
//!
//! - 帳號不存在或密碼錯誤 → `401 UNAUTHORIZED`「帳號或密碼錯誤」（兩者回應完全相同）
//! - 鎖定中 → `403 ACCOUNT_LOCKED`，`details.locked_until` 為解鎖時間
//! - 停用 → `403 ACCOUNT_DISABLED`

use axum::Json;
use axum::extract::State;
use axum::http::header::USER_AGENT;
use axum::http::{HeaderMap, StatusCode};
use hammer_core::audit::client_info;
use hammer_core::auth::{self, AuthContext};
use hammer_shared::auth::{ChangePasswordRequest, CurrentUser, LoginRequest, LoginResponse};
use hammer_shared::constants::CLIENT_VERSION_HEADER;
use time::OffsetDateTime;

use crate::{AppError, AppState, AuthUserAllowPasswordChange};

/// `POST /api/v1/auth/login`
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    let info = request_client_info(&headers);
    let outcome = auth::login(
        &state.pool,
        &req.username,
        &req.password,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(LoginResponse {
        token: outcome.token.into_string(),
        user: current_user(&outcome.user),
    }))
}

/// `POST /api/v1/auth/logout`
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUserAllowPasswordChange,
) -> Result<StatusCode, AppError> {
    let info = request_client_info(&headers);
    auth::logout(
        &state.pool,
        &auth,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/auth/me`
pub async fn me(auth: AuthUserAllowPasswordChange) -> Json<CurrentUser> {
    Json(current_user(&auth))
}

/// `POST /api/v1/auth/change-password`
pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUserAllowPasswordChange,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<StatusCode, AppError> {
    let info = request_client_info(&headers);
    auth::change_password(
        &state.pool,
        &auth,
        &req.current_password,
        &req.new_password,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 由 [`AuthContext`] 組成回應用的使用者資訊。
pub fn current_user(ctx: &AuthContext) -> CurrentUser {
    CurrentUser {
        id: ctx.user_id,
        username: ctx.username.clone(),
        display_name: ctx.display_name.clone(),
        role: ctx.role.clone(),
        permissions: ctx.permissions.iter().cloned().collect(),
        must_change_password: ctx.must_change_password,
    }
}

/// 由 `User-Agent` 與 `X-Client-Version` 組成 `client_info`。
pub(crate) fn request_client_info(headers: &HeaderMap) -> Option<String> {
    let get = |name| headers.get(name).and_then(|v| v.to_str().ok());
    client_info(get(USER_AGENT.as_str()), get(CLIENT_VERSION_HEADER))
}
