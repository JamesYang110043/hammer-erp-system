//! 資料類別與欄位定義（docs/architecture.md §6.1、§7.1、§7.2、§8.5、§9.4）。
//!
//! 讀取需要登入（[`AuthUser`]）；寫入需要 `schema.manage` 權限（在 `hammer_core::schema` 檢查）。
//! request / response 型別、端點清單與錯誤見 [`hammer_shared::schema`]。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use hammer_core::{recalculate, schema};
use hammer_shared::schema::{
    ChangeFieldTypeRequest, CreateEntityRequest, CreateFieldRequest, DeactivateFieldRequest,
    DeleteEntityQuery, EntityDefinition, EntityDetail, FieldDefinition, ListEntitiesQuery,
    RecalculateResult, ReorderEntitiesRequest, ReorderFieldsRequest, UpdateEntityRequest,
    UpdateFieldRequest,
};
use time::OffsetDateTime;

use super::auth::request_client_info;
use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/entities?include_inactive=true`
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ListEntitiesQuery>,
) -> Result<Json<Vec<EntityDefinition>>, AppError> {
    Ok(Json(
        schema::list_entities(&state.pool, &auth, &query).await?,
    ))
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

/// `DELETE /api/v1/entities/{key}?version=N`
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
    Query(query): Query<DeleteEntityQuery>,
) -> Result<StatusCode, AppError> {
    let info = request_client_info(&headers);
    schema::delete_entity(
        &state.pool,
        &auth,
        &key,
        query.version,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/entities/order`
pub async fn reorder(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(req): Json<ReorderEntitiesRequest>,
) -> Result<Json<Vec<EntityDefinition>>, AppError> {
    let info = request_client_info(&headers);
    let entities = schema::reorder_entities(
        &state.pool,
        &auth,
        &req,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(entities))
}

/// 資料類別代碼 `order` 與調整排序的固定路徑相同，路由轉給這些端點（見 `routes::protected_routes`）。
const ORDER_KEY: &str = "order";

/// `GET /api/v1/entities/order`：取得代碼為 `order` 的資料類別。
pub async fn get_order_entity(
    state: State<AppState>,
    auth: AuthUser,
) -> Result<Json<EntityDetail>, AppError> {
    get(state, auth, Path(ORDER_KEY.to_owned())).await
}

/// `PATCH /api/v1/entities/order`：修改代碼為 `order` 的資料類別。
pub async fn update_order_entity(
    state: State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    req: Json<UpdateEntityRequest>,
) -> Result<Json<EntityDetail>, AppError> {
    update(state, headers, auth, Path(ORDER_KEY.to_owned()), req).await
}

/// `DELETE /api/v1/entities/order?version=N`：刪除代碼為 `order` 的資料類別。
pub async fn delete_order_entity(
    state: State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    query: Query<DeleteEntityQuery>,
) -> Result<StatusCode, AppError> {
    delete(state, headers, auth, Path(ORDER_KEY.to_owned()), query).await
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

/// `POST /api/v1/entities/{key}/recalculate`：重新計算資料類別所有資料的公式欄位。
pub async fn recalculate(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(key): Path<String>,
) -> Result<Json<RecalculateResult>, AppError> {
    let info = request_client_info(&headers);
    let result = recalculate::recalculate_records(
        &state.pool,
        &auth,
        &key,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(result))
}
