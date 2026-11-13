//! 自訂資料類別的資料（docs/architecture.md §6.1、§7.1、§8.5 `custom_records`、§9.4、§10）。
//!
//! 讀取需要 `record.read`，寫入需要 `record.write`（在 `hammer_core::records` 檢查）。
//! request / response 型別、端點清單、query string 格式與錯誤見 [`hammer_shared::record`]。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use hammer_core::records;
use hammer_shared::Id;
use hammer_shared::pagination::Page;
use hammer_shared::record::{CreateRecordRequest, DeleteRecordQuery, Record, UpdateRecordRequest};
use time::OffsetDateTime;

use super::auth::request_client_info;
use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/entities/{key}/records`
///
/// 參數名稱是動態的（`filter.<field_key>.<op>`），以 `(名稱, 值)` 清單交給 core 解析。
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(key): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Page<Record>>, AppError> {
    Ok(Json(
        records::list_records(&state.pool, &auth, &key, &pairs).await?,
    ))
}

/// `GET /api/v1/entities/{key}/records/{id}`
pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((key, id)): Path<(String, Id)>,
) -> Result<Json<Record>, AppError> {
    Ok(Json(
        records::get_record(&state.pool, &auth, &key, id).await?,
    ))
}

/// `POST /api/v1/entities/{key}/records`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    Json(req): Json<CreateRecordRequest>,
) -> Result<(StatusCode, Json<Record>), AppError> {
    let info = request_client_info(&headers);
    let record = records::create_record(
        &state.pool,
        &auth,
        &key,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(record)))
}

/// `PATCH /api/v1/entities/{key}/records/{id}`
pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path((key, id)): Path<(String, Id)>,
    Json(req): Json<UpdateRecordRequest>,
) -> Result<Json<Record>, AppError> {
    let info = request_client_info(&headers);
    let record = records::update_record(
        &state.pool,
        &auth,
        &key,
        id,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(record))
}

/// `DELETE /api/v1/entities/{key}/records/{id}?version=N`
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path((key, id)): Path<(String, Id)>,
    Query(query): Query<DeleteRecordQuery>,
) -> Result<StatusCode, AppError> {
    let info = request_client_info(&headers);
    records::delete_record(
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
