//! 帳號管理（docs/architecture.md §6.1、§6.2、§9.4）。
//!
//! 全部需要登入（[`AuthUser`]）與 `user.manage` 權限（在 `hammer_core::users` 檢查）。
//! request / response 型別與錯誤見 [`hammer_shared::user`]。
//!
//! | 端點 | 成功回應 |
//! |---|---|
//! | `GET /api/v1/roles` | `200` `Role[]` |
//! | `GET /api/v1/users` | `200` `User[]` |
//! | `GET /api/v1/users/{id}` | `200` [`User`] |
//! | `POST /api/v1/users` | `201` [`User`] |
//! | `PATCH /api/v1/users/{id}` | `200` [`User`] |
//! | `POST /api/v1/users/{id}/reset-password` | `200` [`User`] |
//! | `POST /api/v1/users/{id}/unlock` | `200` [`User`] |
//! | `DELETE /api/v1/users/{id}?version=N` | `204` |

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use hammer_core::users::{self, NewUserInput, UserChanges};
use hammer_shared::Id;
use hammer_shared::user::{
    CreateUserRequest, DeleteUserQuery, ResetPasswordRequest, Role, UpdateUserRequest, User,
};
use time::OffsetDateTime;

use super::auth::request_client_info;
use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/roles`
pub async fn list_roles(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<Role>>, AppError> {
    Ok(Json(users::list_roles(&state.pool, &auth).await?))
}

/// `GET /api/v1/users`
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<User>>, AppError> {
    let list = users::list_users(&state.pool, &auth, OffsetDateTime::now_utc()).await?;
    Ok(Json(list))
}

/// `GET /api/v1/users/{id}`
pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Id>,
) -> Result<Json<User>, AppError> {
    let user = users::get_user(&state.pool, &auth, id, OffsetDateTime::now_utc()).await?;
    Ok(Json(user))
}

/// `POST /api/v1/users`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<User>), AppError> {
    let info = request_client_info(&headers);
    let user = users::create_user(
        &state.pool,
        &auth,
        &NewUserInput {
            username: &req.username,
            display_name: &req.display_name,
            role: &req.role,
            password: &req.password,
        },
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(user)))
}

/// `PATCH /api/v1/users/{id}`
pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<User>, AppError> {
    let info = request_client_info(&headers);
    let user = users::update_user(
        &state.pool,
        &auth,
        id,
        &UserChanges {
            version: req.version,
            display_name: req.display_name.as_deref(),
            role: req.role.as_deref(),
            is_active: req.is_active,
        },
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(user))
}

/// `POST /api/v1/users/{id}/reset-password`
pub async fn reset_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<User>, AppError> {
    let info = request_client_info(&headers);
    let user = users::reset_password(
        &state.pool,
        &auth,
        id,
        &req.new_password,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(user))
}

/// `POST /api/v1/users/{id}/unlock`
pub async fn unlock(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<Id>,
) -> Result<Json<User>, AppError> {
    let info = request_client_info(&headers);
    let user = users::unlock_user(
        &state.pool,
        &auth,
        id,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(user))
}

/// `DELETE /api/v1/users/{id}?version=N`
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<Id>,
    Query(query): Query<DeleteUserQuery>,
) -> Result<StatusCode, AppError> {
    let info = request_client_info(&headers);
    users::delete_user(
        &state.pool,
        &auth,
        id,
        query.version,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
