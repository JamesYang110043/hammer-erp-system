//! 資料類別與欄位定義（docs/architecture.md §6.1、§7.1、§7.2、§8.5、§9.4）。
//!
//! 讀取需要登入（[`AuthUser`]）；寫入需要 `schema.manage` 權限（在 `hammer_core::schema` 檢查）。
//! request / response 型別、端點清單與錯誤見 [`hammer_shared::schema`]。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use hammer_core::schema;
use hammer_shared::schema::{
    ChangeFieldTypeRequest, CreateEntityRequest, CreateFieldRequest, DeactivateFieldRequest,
    EntityDefinition, EntityDetail, FieldDefinition, ReorderFieldsRequest, UpdateEntityRequest,
    UpdateFieldRequest,
};
use time::OffsetDateTime;

use super::auth::request_client_info;
use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/entities`
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<EntityDefinition>>, AppError> {
    Ok(Json(schema::list_entities(&state.pool, &auth).await?))
}

/// `GET /api/v1/entities/{key}`
pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(key): Path<String>,
) -> Result<Json<EntityDetail>, AppError> {
    Ok(Json(schema::get_entity(&state.pool, &auth, &key).await?))
}

/// `POST /api/v1/entities`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(req): Json<CreateEntityRequest>,
) -> Result<(StatusCode, Json<EntityDetail>), AppError> {
    let info = request_client_info(&headers);
    let detail = schema::create_entity(
        &state.pool,
        &auth,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

/// `PATCH /api/v1/entities/{key}`
pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    Json(req): Json<UpdateEntityRequest>,
) -> Result<Json<EntityDetail>, AppError> {
    let info = request_client_info(&headers);
    let detail = schema::update_entity(
        &state.pool,
        &auth,
        &key,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(detail))
}

/// `POST /api/v1/entities/{key}/fields`
pub async fn create_field(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    Json(req): Json<CreateFieldRequest>,
) -> Result<(StatusCode, Json<FieldDefinition>), AppError> {
    let info = request_client_info(&headers);
    let field = schema::create_field(
        &state.pool,
        &auth,
        &key,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(field)))
}

/// `PATCH /api/v1/entities/{key}/fields/{field_key}`
pub async fn update_field(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path((key, field_key)): Path<(String, String)>,
    Json(req): Json<UpdateFieldRequest>,
) -> Result<Json<FieldDefinition>, AppError> {
    let info = request_client_info(&headers);
    let field = schema::update_field(
        &state.pool,
        &auth,
        &key,
        &field_key,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(field))
}

/// `PATCH /api/v1/entities/{key}/fields/order`：修改代碼為 `order` 的欄位。
///
/// 路由的固定路徑 `order`（調整排序）優先於 `{field_key}`，因此另外接上。
pub async fn update_order_field(
    state: State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    req: Json<UpdateFieldRequest>,
) -> Result<Json<FieldDefinition>, AppError> {
    update_field(state, headers, auth, Path((key, "order".to_owned())), req).await
}

/// `POST /api/v1/entities/{key}/fields/{field_key}/deactivate`
pub async fn deactivate_field(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path((key, field_key)): Path<(String, String)>,
    Json(req): Json<DeactivateFieldRequest>,
) -> Result<Json<FieldDefinition>, AppError> {
    let info = request_client_info(&headers);
    let field = schema::deactivate_field(
        &state.pool,
        &auth,
        &key,
        &field_key,
        req.version,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(field))
}

/// `POST /api/v1/entities/{key}/fields/{field_key}/change-type`
pub async fn change_field_type(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path((key, field_key)): Path<(String, String)>,
    Json(req): Json<ChangeFieldTypeRequest>,
) -> Result<Json<FieldDefinition>, AppError> {
    let info = request_client_info(&headers);
    let field = schema::change_field_type(
        &state.pool,
        &auth,
        &key,
        &field_key,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(field))
}

/// `PUT /api/v1/entities/{key}/fields/order`
pub async fn reorder_fields(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    Json(req): Json<ReorderFieldsRequest>,
) -> Result<Json<Vec<FieldDefinition>>, AppError> {
    let info = request_client_info(&headers);
    let fields = schema::reorder_fields(
        &state.pool,
        &auth,
        &key,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(fields))
}
