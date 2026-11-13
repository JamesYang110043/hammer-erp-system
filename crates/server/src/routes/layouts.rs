//! 列表版面（docs/architecture.md §6.1、§8.5 `view_layouts`、§9.4、§10）。
//!
//! 權限檢查在 `hammer_core::layouts`。request / response 型別、端點清單、權限與錯誤見
//! [`hammer_shared::layout`]。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use hammer_core::layouts;
use hammer_shared::Id;
use hammer_shared::layout::{
    CreateViewLayoutRequest, DefaultViewLayout, DeleteViewLayoutQuery, UpdateViewLayoutRequest,
    ViewLayout,
};
use time::OffsetDateTime;

use super::auth::request_client_info;
use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/entities/{key}/layouts`
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(key): Path<String>,
) -> Result<Json<Vec<ViewLayout>>, AppError> {
    Ok(Json(layouts::list_layouts(&state.pool, &auth, &key).await?))
}

/// `GET /api/v1/entities/{key}/layouts/default`
pub async fn get_default(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(key): Path<String>,
) -> Result<Json<DefaultViewLayout>, AppError> {
    Ok(Json(
        layouts::get_default_layout(&state.pool, &auth, &key).await?,
    ))
}

/// `POST /api/v1/entities/{key}/layouts`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    Json(req): Json<CreateViewLayoutRequest>,
) -> Result<(StatusCode, Json<ViewLayout>), AppError> {
    let info = request_client_info(&headers);
    let layout = layouts::create_layout(
        &state.pool,
        &auth,
        &key,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(layout)))
}

/// `PATCH /api/v1/entities/{key}/layouts/{id}`
pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path((key, id)): Path<(String, Id)>,
    Json(req): Json<UpdateViewLayoutRequest>,
) -> Result<Json<ViewLayout>, AppError> {
    let info = request_client_info(&headers);
    let layout = layouts::update_layout(
        &state.pool,
        &auth,
        &key,
        id,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(layout))
}

/// `DELETE /api/v1/entities/{key}/layouts/{id}?version=N`
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path((key, id)): Path<(String, Id)>,
    Query(query): Query<DeleteViewLayoutQuery>,
) -> Result<StatusCode, AppError> {
    let info = request_client_info(&headers);
    layouts::delete_layout(
        &state.pool,
        &auth,
        &key,
        id,
        query.version,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
