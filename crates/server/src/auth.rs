//! 認證 extractor（docs/architecture.md §6.2、§10）。
//!
//! 需要登入的 handler 加上 [`AuthUser`] 參數即可，權限用 `auth.require(...)` 檢查：
//!
//! ```ignore
//! async fn handler(auth: AuthUser) -> Result<Json<T>, AppError> {
//!     auth.require(permission::RECORD_WRITE)?;
//!     …
//! }
//! ```
//!
//! - 沒帶 token、格式錯誤、token 無效、session 失效、使用者停用或刪除 → `401 UNAUTHORIZED`
//! - [`AuthUser`]：`must_change_password` 時回傳 `403 PASSWORD_CHANGE_REQUIRED`
//! - [`AuthUserAllowPasswordChange`]：不檢查 `must_change_password`，
//!   **只給**改密碼、登出、`/auth/me` 使用
//! - 權限不足 → `403 FORBIDDEN`（`details.permission` 為缺少的權限代碼）
//!
//! 不需要登入的路由見 [`crate::routes`]。

use std::ops::Deref;

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use hammer_core::CoreError;
use hammer_core::auth::{AuthContext, authenticate};
use time::OffsetDateTime;

use crate::{AppError, AppState};

/// 已登入、且不需要先修改密碼的使用者。大部分 API 使用這個。
#[derive(Debug, Clone)]
pub struct AuthUser(pub AuthContext);

/// 已登入的使用者，不檢查 `must_change_password`。
///
/// 只用於強制改密碼期間仍要能呼叫的 API：修改密碼、登出、`/auth/me`。
#[derive(Debug, Clone)]
pub struct AuthUserAllowPasswordChange(pub AuthContext);

impl Deref for AuthUser {
    type Target = AuthContext;

    fn deref(&self) -> &AuthContext {
        &self.0
    }
}

impl Deref for AuthUserAllowPasswordChange {
    type Target = AuthContext;

    fn deref(&self) -> &AuthContext {
        &self.0
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let ctx = current_user(parts, state).await?;
        ctx.ensure_password_changed()?;
        Ok(Self(ctx))
    }
}

impl FromRequestParts<AppState> for AuthUserAllowPasswordChange {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        Ok(Self(current_user(parts, state).await?))
    }
}

/// 驗證請求的 token。同一個請求只查一次資料庫，結果存在 request extensions。
async fn current_user(parts: &mut Parts, state: &AppState) -> Result<AuthContext, AppError> {
    if let Some(ctx) = parts.extensions.get::<AuthContext>() {
        return Ok(ctx.clone());
    }
    let token = parts
        .headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(bearer_token)
        .ok_or(CoreError::Unauthorized)?;
    let ctx = authenticate(&state.pool, token, OffsetDateTime::now_utc()).await?;
    parts.extensions.insert(ctx.clone());
    Ok(ctx)
}

/// 從 `Authorization` 標頭值取出 bearer token。scheme 不分大小寫（RFC 7235）。
pub fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bearer_token() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer abc"), Some("abc"));
        assert_eq!(bearer_token("BEARER  abc "), Some("abc"));
    }

    #[test]
    fn rejects_invalid_headers() {
        assert_eq!(bearer_token(""), None);
        assert_eq!(bearer_token("Bearer"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("abc"), None);
    }
}
