//! 表單版面（docs/architecture.md §7.7、§8.5 `form_layouts`、§9.4、§10）。
//!
//! 權限檢查在 `hammer_core::form_layouts`。request / response 型別、端點清單、權限與錯誤見
//! [`hammer_shared::form_layout`]。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use hammer_core::form_layouts;
use hammer_shared::form_layout::{DeleteFormLayoutQuery, FormLayout, SaveFormLayoutRequest};
use time::OffsetDateTime;

use super::auth::request_client_info;
use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/entities/{key}/form-layout`
pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(key): Path<String>,
) -> Result<Json<FormLayout>, AppError> {
    Ok(Json(
        form_layouts::get_form_layout(&state.pool, &auth, &key).await?,
    ))
}

/// `PUT /api/v1/entities/{key}/form-layout`
pub async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    Json(req): Json<SaveFormLayoutRequest>,
) -> Result<Json<FormLayout>, AppError> {
    let info = request_client_info(&headers);
    let layout = form_layouts::save_form_layout(
        &state.pool,
        &auth,
        &key,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(layout))
}

/// `DELETE /api/v1/entities/{key}/form-layout?version=N`
pub async fn reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    Query(query): Query<DeleteFormLayoutQuery>,
) -> Result<Json<FormLayout>, AppError> {
    let info = request_client_info(&headers);
    let layout = form_layouts::reset_form_layout(
        &state.pool,
        &auth,
        &key,
        query.version,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(layout))
}
