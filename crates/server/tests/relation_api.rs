//! 關聯欄位（`relation`）的 API 整合測試（#70）。
//!
//! - 欄位定義：目標只能是自訂、有效、`master` / `document` 的資料類別；建立後不可修改；
//!   型態變更只能轉成 `text` / `long_text`
//! - 寫入：值必須是目標資料類別中未刪除的資料 id
//! - 回應：`relations` 列出目標資料的標題與是否已刪除
//! - 篩選 `eq`、`empty`；不能排序
//! - 選擇器搜尋 `GET /api/v1/entities/{key}/records/lookup`
//!
//! 純邏輯（變動判斷、錯誤彙整、設定檢查）在 `hammer_core::relations`、`hammer_core::schema::rules`
//! 的單元測試。使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::pagination::Page;
use hammer_shared::record::{LookupItem, Record};
use hammer_shared::schema::{EntityDetail, FieldDefinition};
use hammer_shared::{ApiError, ErrorCode, Id, new_id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// 被關聯的資料類別（設備）。
const EQUIPMENT: &str = "equipment";
/// 有關聯欄位的資料類別（維修單）。
const REPAIR: &str = "repair_order";
/// 其他資料類別（用來測試「屬於其他資料類別的 id」）。
const OTHER: &str = "vendor";

fn app(pool: &PgPool) -> Router {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    router(AppState::new(pool.clone(), config))
}

async fn call(
    pool: &PgPool,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0")
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(json) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let res = app(pool).oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

fn assert_error(status: StatusCode, body: &Value, expected: ErrorCode) -> ApiError {
    assert_eq!(status.as_u16(), expected.http_status(), "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, expected, "{body}");
    assert!(!err.message.is_empty());
    err
}

/// 422 VALIDATION，且 `details.field` 為 `field`。
fn assert_invalid(status: StatusCode, body: &Value, field: &str) {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert_eq!(err.details.unwrap()["field"], json!(field), "{body}");
}

/// 422 VALIDATION，且 `details.fields` 的 `field_key` 依序為 `keys`。
fn assert_invalid_fields(status: StatusCode, body: &Value, keys: &[&str]) {
    let err = assert_error(status, body, ErrorCode::Validation);
    let fields = err.details.unwrap()["fields"].as_array().unwrap().clone();
    let actual: Vec<_> = fields
        .iter()
        .map(|f| {
            assert!(!f["message"].as_str().unwrap().is_empty());
            f["field_key"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(actual, keys, "{body}");
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

async fn create_entity(pool: &PgPool, admin: &str, key: &str) -> EntityDetail {
    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        admin,
        Some(json!({ "key": key, "label": key, "kind": "master" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_field_raw(
    pool: &PgPool,
    admin: &str,
    entity: &str,
    field: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        admin,
        Some(field),
    )
    .await
}

async fn create_field(pool: &PgPool, admin: &str, entity: &str, field: Value) -> FieldDefinition {
    let (status, body) = create_field_raw(pool, admin, entity, field).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

fn relation_field(key: &str, target: Id) -> Value {
    json!({
        "field_key": key,
        "label": key,
        "data_type": "relation",
        "relation_entity_id": target,
    })
}

/// 建立以下資料類別，回傳 `(管理者 token, 設備的 id)`。
///
/// | 資料類別 | 欄位 |
/// |---|---|
/// | [`EQUIPMENT`] | `name`（text，標題的預設規則使用它） |
/// | [`OTHER`] | `name`（text） |
/// | [`REPAIR`] | `note`（text）、`equipment`（relation → [`EQUIPMENT`]） |
async fn setup(pool: &PgPool) -> (String, Id) {
    let admin = token_for(pool, "admin", role::ADMIN).await;
    let equipment = create_entity(pool, &admin, EQUIPMENT).await.entity.id;
    let name = json!({ "field_key": "name", "label": "名稱", "data_type": "text" });
    create_field(pool, &admin, EQUIPMENT, name.clone()).await;
    create_entity(pool, &admin, OTHER).await;
    create_field(pool, &admin, OTHER, name).await;
    create_entity(pool, &admin, REPAIR).await;
    create_field(
        pool,
        &admin,
        REPAIR,
        json!({ "field_key": "note", "label": "備註", "data_type": "text" }),
    )
    .await;
    create_field(pool, &admin, REPAIR, relation_field("equipment", equipment)).await;
    (admin, equipment)
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

fn record_uri(entity: &str, id: Id) -> String {
    format!("/api/v1/entities/{entity}/records/{id}")
}

async fn create_raw(pool: &PgPool, token: &str, entity: &str, data: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &records_uri(entity),
        token,
        Some(json!({ "data": data })),
    )
    .await
}

async fn create(pool: &PgPool, token: &str, entity: &str, data: Value) -> Record {
    let (status, body) = create_raw(pool, token, entity, data).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(entity, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn delete(pool: &PgPool, token: &str, entity: &str, record: &Record) {
    let (status, body) = call(
        pool,
        Method::DELETE,
        &format!(
            "{}?version={}",
            record_uri(entity, record.id),
            record.version
        ),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// URL 編碼（query string 的值）。
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn query_string(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn list_raw(pool: &PgPool, token: &str, params: &[(&str, &str)]) -> (StatusCode, Value) {
    let uri = format!("{}?{}", records_uri(REPAIR), query_string(params));
    call(pool, Method::GET, &uri, token, None).await
}

async fn list_ids(pool: &PgPool, token: &str, params: &[(&str, &str)]) -> Vec<Id> {
    let (status, body) = list_raw(pool, token, params).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    page.items.into_iter().map(|r| r.id).collect()
}

async fn lookup_raw(pool: &PgPool, token: &str, params: &[(&str, &str)]) -> (StatusCode, Value) {
    let uri = format!(
        "/api/v1/entities/{EQUIPMENT}/records/lookup?{}",
        query_string(params)
    );
    call(pool, Method::GET, &uri, token, None).await
}

async fn lookup(pool: &PgPool, token: &str, params: &[(&str, &str)]) -> Vec<LookupItem> {
    let (status, body) = lookup_raw(pool, token, params).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

// ---------------------------------------------------------------------------
// 欄位定義
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn relation_field_is_created_with_target(pool: PgPool) {
    let (admin, equipment) = setup(&pool).await;
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{REPAIR}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    let field = detail
        .fields
        .iter()
        .find(|f| f.field_key == "equipment")
        .unwrap();
    assert_eq!(field.relation_entity_id, Some(equipment));

    // 可以關聯到自己
    let repair = detail.entity.id;
    let parent = create_field(&pool, &admin, REPAIR, relation_field("parent", repair)).await;
    assert_eq!(parent.relation_entity_id, Some(repair));

    // 可搜尋、不可重複可以開啟（動態索引）
    let unique = create_field(
        &pool,
        &admin,
        REPAIR,
        json!({
            "field_key": "main_equipment",
            "label": "主要設備",
            "data_type": "relation",
            "relation_entity_id": equipment,
            "is_searchable": true,
            "is_unique": true,
        }),
    )
    .await;
    assert!(unique.is_searchable && unique.is_unique);
    let eq = create(&pool, &admin, EQUIPMENT, json!({ "name": "A" })).await;
    create(&pool, &admin, REPAIR, json!({ "main_equipment": eq.id })).await;
    let (status, body) =
        create_raw(&pool, &admin, REPAIR, json!({ "main_equipment": eq.id })).await;
    assert_invalid_fields(status, &body, &["main_equipment"]);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn relation_target_must_be_active_custom_master_or_document(pool: PgPool) {
    let (admin, equipment) = setup(&pool).await;

    // 沒有指定目標、目標不存在
    let (status, body) = create_field_raw(
        &pool,
        &admin,
        REPAIR,
        json!({ "field_key": "x", "label": "x", "data_type": "relation" }),
    )
    .await;
    assert_invalid(status, &body, "relation_entity_id");
    let (status, body) =
        create_field_raw(&pool, &admin, REPAIR, relation_field("x", new_id())).await;
    assert_invalid(status, &body, "relation_entity_id");

    // 明細子表：API 目前不能建立（#72），直接寫入資料庫
    let detail_id = new_id();
    sqlx::query(
        "INSERT INTO entity_definitions (id, key, label, kind, storage, parent_entity_id) \
         VALUES ($1, 'equipment_parts', '零件', 'detail', 'custom_records', $2)",
    )
    .bind(detail_id)
    .bind(equipment)
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) =
        create_field_raw(&pool, &admin, REPAIR, relation_field("x", detail_id)).await;
    assert_invalid(status, &body, "relation_entity_id");

    // 系統資料類別（M3）
    let system_id = new_id();
    sqlx::query(
        "INSERT INTO entity_definitions (id, key, label, kind, is_system, storage, table_name) \
         VALUES ($1, 'customer', '客戶', 'master', true, 'table', 'customers')",
    )
    .bind(system_id)
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) =
        create_field_raw(&pool, &admin, REPAIR, relation_field("x", system_id)).await;
    assert_invalid(status, &body, "relation_entity_id");

    // 單據（document）可以
    let document_id = new_id();
    sqlx::query(
        "INSERT INTO entity_definitions (id, key, label, kind, storage) \
         VALUES ($1, 'work_order', '工單', 'document', 'custom_records')",
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .unwrap();
    create_field(
        &pool,
        &admin,
        REPAIR,
        relation_field("work_order", document_id),
    )
    .await;

    // 停用的資料類別
    let (_, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{OTHER}"),
        &admin,
        None,
    )
    .await;
    let other: EntityDetail = serde_json::from_value(body).unwrap();
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &format!("/api/v1/entities/{OTHER}"),
        &admin,
        Some(json!({
            "version": other.entity.version,
            "label": other.entity.label,
            "icon": null,
            "title_template": null,
            "sort_order": 0,
            "is_active": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) =
        create_field_raw(&pool, &admin, REPAIR, relation_field("x", other.entity.id)).await;
    assert_invalid(status, &body, "relation_entity_id");

    // 其他型態不可指定目標；關聯欄位不可設定預設值
    let (status, body) = create_field_raw(
        &pool,
        &admin,
        REPAIR,
        json!({ "field_key": "x", "label": "x", "data_type": "text", "relation_entity_id": equipment }),
    )
    .await;
    assert_invalid(status, &body, "relation_entity_id");
    let eq = create(&pool, &admin, EQUIPMENT, json!({ "name": "A" })).await;
    let mut with_default = relation_field("x", equipment);
    with_default["default_value"] = json!(eq.id);
    let (status, body) = create_field_raw(&pool, &admin, REPAIR, with_default).await;
    assert_invalid(status, &body, "default_value");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn relation_target_cannot_be_changed(pool: PgPool) {
    let (admin, equipment) = setup(&pool).await;
    let (_, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{OTHER}"),
        &admin,
        None,
    )
    .await;
    let other: EntityDetail = serde_json::from_value(body).unwrap();
    let field = create_field(&pool, &admin, REPAIR, relation_field("x", equipment)).await;
    let update = |target: Option<Id>| {
        json!({
            "version": field.version,
            "label": "新名稱",
            "data_type": "relation",
            "is_required": false,
            "is_visible": true,
            "is_readonly": false,
            "is_searchable": false,
            "is_unique": false,
            "default_value": null,
            "validation": null,
            "options": null,
            "decimal_places": null,
            "relation_entity_id": target,
            "ui_widget": null,
            "column_width": null,
            "help_text": null,
            "sort_order": 0,
            "is_active": true,
        })
    };
    let uri = format!("/api/v1/entities/{REPAIR}/fields/x");
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &admin,
        Some(update(Some(other.entity.id))),
    )
    .await;
    assert_invalid(status, &body, "relation_entity_id");

    // 省略（null）時保留原本的目標
    let (status, body) = call(&pool, Method::PATCH, &uri, &admin, Some(update(None))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(updated.label, "新名稱");
    assert_eq!(updated.relation_entity_id, Some(equipment));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn relation_changes_only_to_text(pool: PgPool) {
    let (admin, equipment) = setup(&pool).await;
    let eq = create(&pool, &admin, EQUIPMENT, json!({ "name": "A" })).await;
    let repair = create(&pool, &admin, REPAIR, json!({ "equipment": eq.id })).await;
    let (_, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{REPAIR}"),
        &admin,
        None,
    )
    .await;
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    let field = |key: &str| {
        detail
            .fields
            .iter()
            .find(|f| f.field_key == key)
            .unwrap()
            .clone()
    };
    let change = |key: &str, version: i32, data_type: &str| {
        (
            format!("/api/v1/entities/{REPAIR}/fields/{key}/change-type"),
            json!({ "version": version, "data_type": data_type }),
        )
    };

    // 其他型態不能轉成關聯
    let note = field("note");
    let (uri, req) = change("note", note.version, "relation");
    let (status, body) = call(&pool, Method::POST, &uri, &admin, Some(req)).await;
    assert_invalid(status, &body, "data_type");

    // 關聯不能轉成文字以外的型態
    let rel = field("equipment");
    assert_eq!(rel.relation_entity_id, Some(equipment));
    for t in ["integer", "date", "boolean"] {
        let (uri, req) = change("equipment", rel.version, t);
        let (status, body) = call(&pool, Method::POST, &uri, &admin, Some(req)).await;
        assert_invalid(status, &body, "data_type");
    }

    // 轉成文字：值為資料 id 的文字，清除目標資料類別
    let (uri, req) = change("equipment", rel.version, "long_text");
    let (status, body) = call(&pool, Method::POST, &uri, &admin, Some(req)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let changed: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(changed.relation_entity_id, None);
    let after = get(&pool, &admin, REPAIR, repair.id).await;
    assert_eq!(after.data["equipment"], json!(eq.id.to_string()));
    assert!(after.relations.is_empty());
}

// ---------------------------------------------------------------------------
// 寫入與讀取
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn write_rejects_missing_deleted_and_foreign_ids(pool: PgPool) {
    let (admin, _) = setup(&pool).await;
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let alive = create(&pool, &admin, EQUIPMENT, json!({ "name": "A" })).await;
    let deleted = create(&pool, &admin, EQUIPMENT, json!({ "name": "B" })).await;
    delete(&pool, &admin, EQUIPMENT, &deleted).await;
    let foreign = create(&pool, &admin, OTHER, json!({ "name": "C" })).await;

    for id in [new_id(), deleted.id, foreign.id] {
        let (status, body) = create_raw(&pool, &editor, REPAIR, json!({ "equipment": id })).await;
        assert_invalid_fields(status, &body, &["equipment"]);
    }
    // 格式不正確
    let (status, body) = create_raw(
        &pool,
        &editor,
        REPAIR,
        json!({ "equipment": "abc", "note": "n" }),
    )
    .await;
    assert_invalid_fields(status, &body, &["equipment"]);

    // 合法的 id：回應包含關聯資訊
    let repair = create(&pool, &editor, REPAIR, json!({ "equipment": alive.id })).await;
    assert_eq!(repair.data["equipment"], json!(alive.id.to_string()));
    let r = &repair.relations["equipment"];
    assert_eq!((r.id, r.title.as_str(), r.deleted), (alive.id, "A", false));

    // 修改成不合法的 id
    for id in [new_id(), deleted.id, foreign.id] {
        let (status, body) = call(
            &pool,
            Method::PATCH,
            &record_uri(REPAIR, repair.id),
            &editor,
            Some(json!({ "version": repair.version, "data": { "equipment": id } })),
        )
        .await;
        assert_invalid_fields(status, &body, &["equipment"]);
    }
    // 清空關聯
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(REPAIR, repair.id),
        &editor,
        Some(json!({ "version": repair.version, "data": { "equipment": null } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cleared: Record = serde_json::from_value(body).unwrap();
    assert!(!cleared.data.contains_key("equipment"));
    assert!(cleared.relations.is_empty());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn deleted_target_keeps_value_and_is_marked(pool: PgPool) {
    let (admin, _) = setup(&pool).await;
    let eq = create(&pool, &admin, EQUIPMENT, json!({ "name": "冷氣 A" })).await;
    let repair = create(
        &pool,
        &admin,
        REPAIR,
        json!({ "equipment": eq.id, "note": "n" }),
    )
    .await;

    // 被關聯的資料可以刪除
    delete(&pool, &admin, EQUIPMENT, &eq).await;

    // 單筆與列表都標示為已刪除，保留原值與刪除前的標題
    let after = get(&pool, &admin, REPAIR, repair.id).await;
    assert_eq!(after.data["equipment"], json!(eq.id.to_string()));
    let r = &after.relations["equipment"];
    assert_eq!((r.id, r.title.as_str(), r.deleted), (eq.id, "冷氣 A", true));
    let (status, body) = list_raw(&pool, &admin, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    assert!(page.items[0].relations["equipment"].deleted);

    // 修改其他欄位（含原樣送回關聯值）仍可儲存
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(REPAIR, repair.id),
        &admin,
        Some(json!({
            "version": after.version,
            "data": { "note": "改過", "equipment": eq.id },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: Record = serde_json::from_value(body).unwrap();
    assert_eq!(updated.data["note"], json!("改過"));
    assert!(updated.relations["equipment"].deleted);
}

// ---------------------------------------------------------------------------
// 篩選
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn relation_filters_eq_and_empty(pool: PgPool) {
    let (admin, _) = setup(&pool).await;
    let a = create(&pool, &admin, EQUIPMENT, json!({ "name": "A" })).await;
    let b = create(&pool, &admin, EQUIPMENT, json!({ "name": "B" })).await;
    let r1 = create(&pool, &admin, REPAIR, json!({ "equipment": a.id })).await;
    let r2 = create(&pool, &admin, REPAIR, json!({ "equipment": b.id })).await;
    let r3 = create(&pool, &admin, REPAIR, json!({ "note": "沒有設備" })).await;
    let r4 = create(&pool, &admin, REPAIR, json!({ "equipment": a.id })).await;

    let a_id = a.id.to_string();
    assert_eq!(
        list_ids(&pool, &admin, &[("filter.equipment.eq", &a_id)]).await,
        [r4.id, r1.id]
    );
    // 大寫的 id 也可以
    assert_eq!(
        list_ids(
            &pool,
            &admin,
            &[("filter.equipment.eq", &b.id.to_string().to_uppercase())]
        )
        .await,
        [r2.id]
    );
    assert_eq!(
        list_ids(&pool, &admin, &[("filter.equipment.empty", "true")]).await,
        [r3.id]
    );
    assert_eq!(
        list_ids(&pool, &admin, &[("filter.equipment.empty", "false")]).await,
        [r4.id, r2.id, r1.id]
    );

    // 不支援的運算子、格式錯誤的 id、排序
    for (param, value) in [
        ("filter.equipment.contains", "A"),
        ("filter.equipment.gt", a_id.as_str()),
        ("filter.equipment.eq", "abc"),
        ("sort", "equipment"),
    ] {
        let (status, body) = list_raw(&pool, &admin, &[(param, value)]).await;
        assert_invalid(status, &body, param);
    }
}

// ---------------------------------------------------------------------------
// 選擇器搜尋
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn lookup_matches_titles_and_skips_deleted(pool: PgPool) {
    let (admin, _) = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let taipei_pump = create(&pool, &admin, EQUIPMENT, json!({ "name": "台北 Pump" })).await;
    let kaohsiung_pump = create(&pool, &admin, EQUIPMENT, json!({ "name": "高雄 pump" })).await;
    let taipei_fan = create(&pool, &admin, EQUIPMENT, json!({ "name": "台北 Fan" })).await;
    let deleted_pump = create(&pool, &admin, EQUIPMENT, json!({ "name": "舊 Pump" })).await;
    delete(&pool, &admin, EQUIPMENT, &deleted_pump).await;

    let ids = |items: Vec<LookupItem>| items.into_iter().map(|i| i.id).collect::<Vec<_>>();

    // 不分大小寫、不含已刪除，由新到舊；只需要 record.read（檢視者）
    let items = lookup(&pool, &viewer, &[("q", "PUMP")]).await;
    assert_eq!(
        items,
        [
            LookupItem {
                id: kaohsiung_pump.id,
                title: "高雄 pump".into(),
            },
            LookupItem {
                id: taipei_pump.id,
                title: "台北 Pump".into(),
            },
        ]
    );
    // 每個詞都要出現
    assert_eq!(
        ids(lookup(&pool, &viewer, &[("q", "pump 台北")]).await),
        [taipei_pump.id]
    );
    // 沒有關鍵字：全部（未刪除）
    assert_eq!(
        ids(lookup(&pool, &viewer, &[]).await),
        [taipei_fan.id, kaohsiung_pump.id, taipei_pump.id]
    );
    // 筆數上限
    assert_eq!(
        ids(lookup(&pool, &viewer, &[("q", "台北"), ("limit", "1")]).await),
        [taipei_fan.id]
    );
    assert!(lookup(&pool, &viewer, &[("q", "不存在")]).await.is_empty());

    // limit 不合法
    let (status, body) = lookup_raw(&pool, &viewer, &[("limit", "0")]).await;
    assert_invalid(status, &body, "limit");
    let (status, body) = lookup_raw(&pool, &viewer, &[("limit", "abc")]).await;
    assert_error(status, &body, ErrorCode::BadRequest);

    // 資料類別不存在
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/entities/missing/records/lookup",
        &viewer,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);
}
