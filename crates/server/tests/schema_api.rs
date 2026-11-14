//! 資料類別與欄位定義 API 的整合測試（#47）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::schema::{DataType, EntityDefinition, EntityDetail, FieldDefinition};
use hammer_shared::{ApiError, ErrorCode, Id, audit, new_id, permission, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

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
fn assert_invalid_field(status: StatusCode, body: &Value, field: &str) -> ApiError {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": field })), "{body}");
    err
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

async fn admin(pool: &PgPool) -> String {
    token_for(pool, "admin", role::ADMIN).await
}

async fn create_entity(pool: &PgPool, token: &str, key: &str) -> EntityDefinition {
    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        token,
        Some(json!({ "key": key, "label": "維修單", "kind": "master" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    assert!(detail.fields.is_empty());
    detail.entity
}

async fn add_field(pool: &PgPool, token: &str, entity: &str, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        token,
        Some(body),
    )
    .await
}

async fn add_text_field(pool: &PgPool, token: &str, entity: &str, key: &str) -> FieldDefinition {
    let (status, body) = add_field(
        pool,
        token,
        entity,
        json!({ "field_key": key, "label": format!("{key} 名稱"), "data_type": "text" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get_entity(pool: &PgPool, token: &str, key: &str) -> EntityDetail {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{key}"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// `PATCH` 欄位的完整 body（整筆取代），可用 `changes` 覆蓋部分屬性。
fn field_patch(field: &FieldDefinition, changes: Value) -> Value {
    let mut body = json!({
        "version": field.version,
        "label": field.label,
        "data_type": field.data_type,
        "is_required": field.is_required,
        "is_visible": field.is_visible,
        "is_readonly": field.is_readonly,
        "is_searchable": field.is_searchable,
        "is_unique": field.is_unique,
        "default_value": field.default_value,
        "validation": field.validation,
        "options": field.options,
        "decimal_places": field.decimal_places,
        "ui_widget": field.ui_widget,
        "column_width": field.column_width,
        "help_text": field.help_text,
        "sort_order": field.sort_order,
        "is_active": field.is_active,
    });
    for (k, v) in changes.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

fn entity_patch(entity: &EntityDefinition, changes: Value) -> Value {
    let mut body = json!({
        "version": entity.version,
        "label": entity.label,
        "icon": entity.icon,
        "title_template": entity.title_template,
        "sort_order": entity.sort_order,
        "is_active": entity.is_active,
    });
    for (k, v) in changes.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

async fn patch_field(
    pool: &PgPool,
    token: &str,
    entity: &str,
    field: &FieldDefinition,
    changes: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/entities/{entity}/fields/{}", field.field_key),
        token,
        Some(field_patch(field, changes)),
    )
    .await
}

async fn deactivate(
    pool: &PgPool,
    token: &str,
    entity: &str,
    field_key: &str,
    version: i32,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields/{field_key}/deactivate"),
        token,
        Some(json!({ "version": version })),
    )
    .await
}

#[derive(Debug, sqlx::FromRow)]
struct ChangeLog {
    target_type: String,
    action: String,
    changed_by: Option<Id>,
    before_data: Option<Value>,
    after_data: Option<Value>,
}

async fn change_logs(pool: &PgPool, target_id: Id) -> Vec<ChangeLog> {
    sqlx::query_as(
        "SELECT target_type, action, changed_by, before_data, after_data
         FROM schema_change_logs WHERE target_id = $1 ORDER BY changed_at, id",
    )
    .bind(target_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    action: String,
    entity_key: Option<String>,
    before_data: Option<Value>,
    after_data: Option<Value>,
    client_info: Option<String>,
}

async fn audit_logs(pool: &PgPool, record_id: Id) -> Vec<AuditRow> {
    sqlx::query_as(
        "SELECT action, entity_key, before_data, after_data, client_info
         FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    let sql = match table {
        "schema_change_logs" => "SELECT count(*) FROM schema_change_logs",
        "field_definitions" => "SELECT count(*) FROM field_definitions",
        other => panic!("未支援的資料表：{other}"),
    };
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// 直接在資料庫插入一筆系統欄位（`is_system = true`）。
async fn insert_system_field(pool: &PgPool, entity_id: Id, field_key: &str) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO field_definitions
             (id, entity_id, field_key, label, data_type, is_system, storage, column_name, is_required)
         VALUES ($1, $2, $3, '名稱', 'text', true, 'column', $3, true)",
    )
    .bind(id)
    .bind(entity_id)
    .bind(field_key)
    .execute(pool)
    .await
    .unwrap();
    id
}

// ---------------------------------------------------------------------------
// 驗收條件
// ---------------------------------------------------------------------------

/// editor、viewer 呼叫寫入 API 得到 403；讀取 API 可以正常呼叫。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn editor_and_viewer_can_read_but_not_write(pool: PgPool) {
    let admin = admin(&pool).await;
    let entity = create_entity(&pool, &admin, "repair_order").await;
    let field = add_text_field(&pool, &admin, "repair_order", "title").await;
    let logs_before = count(&pool, "schema_change_logs").await;

    for (username, role_code) in [("editor", role::EDITOR), ("viewer", role::VIEWER)] {
        let token = token_for(&pool, username, role_code).await;

        let (status, body) = call(&pool, Method::GET, "/api/v1/entities", &token, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let list: Vec<EntityDefinition> = serde_json::from_value(body).unwrap();
        assert_eq!(list.len(), 1);
        let detail = get_entity(&pool, &token, "repair_order").await;
        assert_eq!(detail.fields.len(), 1);

        let base = "/api/v1/entities/repair_order";
        let requests: Vec<(Method, String, Value)> = vec![
            (
                Method::POST,
                "/api/v1/entities".into(),
                json!({ "key": "other", "label": "其他", "kind": "master" }),
            ),
            (
                Method::PATCH,
                base.into(),
                entity_patch(&entity, json!({ "label": "X" })),
            ),
            (
                Method::POST,
                format!("{base}/fields"),
                json!({ "field_key": "x", "label": "X", "data_type": "text" }),
            ),
            (
                Method::PATCH,
                format!("{base}/fields/title"),
                field_patch(&field, json!({ "label": "X" })),
            ),
            (
                Method::POST,
                format!("{base}/fields/title/deactivate"),
                json!({ "version": field.version }),
            ),
            (
                Method::PUT,
                format!("{base}/fields/order"),
                json!({ "fields": [{ "field_key": "title", "version": field.version }] }),
            ),
        ];
        for (method, uri, body) in requests {
            let (status, res) = call(&pool, method.clone(), &uri, &token, Some(body)).await;
            let err = assert_error(status, &res, ErrorCode::Forbidden);
            assert_eq!(
                err.details,
                Some(json!({ "permission": permission::SCHEMA_MANAGE })),
                "{role_code} {method} {uri}"
            );
        }
    }
    assert_eq!(count(&pool, "schema_change_logs").await, logs_before);
    assert_eq!(
        get_entity(&pool, &admin, "repair_order").await.entity,
        entity
    );
}

/// 建立資料類別 → 新增欄位 → 修改 label → 停用，每一步都有 `schema_change_logs`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn every_change_writes_schema_change_logs(pool: PgPool) {
    let token = admin(&pool).await;
    let admin_id: Id = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();

    // 建立資料類別
    let entity = create_entity(&pool, &token, "repair_order").await;
    assert_eq!(entity.kind, hammer_shared::schema::EntityKind::Master);
    assert_eq!(
        entity.storage,
        hammer_shared::schema::EntityStorage::CustomRecords
    );
    assert!(!entity.is_system && entity.is_active);
    let logs = change_logs(&pool, entity.id).await;
    assert_eq!(logs.len(), 1);
    assert_eq!(
        (logs[0].target_type.as_str(), logs[0].action.as_str()),
        ("entity", "create")
    );
    assert_eq!(logs[0].changed_by, Some(admin_id));
    assert_eq!(logs[0].before_data, None);
    assert_eq!(logs[0].after_data.as_ref().unwrap()["key"], "repair_order");

    // 新增欄位（預設值依型態正規化）
    let (status, body) = add_field(
        &pool,
        &token,
        "repair_order",
        json!({
            "field_key": "fee",
            "label": "費用",
            "data_type": "money",
            "decimal_places": 2,
            "default_value": "100",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let field: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(field.data_type, DataType::Money);
    assert_eq!(field.default_value, Some(json!("100.00")));
    assert_eq!(field.version, 1);

    // 修改 label
    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "label": "維修費" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let renamed: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(renamed.label, "維修費");
    assert_eq!(renamed.version, 2);

    // 停用
    let (status, body) = deactivate(&pool, &token, "repair_order", "fee", renamed.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let inactive: FieldDefinition = serde_json::from_value(body).unwrap();
    assert!(!inactive.is_active);
    assert_eq!(inactive.version, 3);
    // 已停用再停用：不寫入
    let (status, body) = deactivate(&pool, &token, "repair_order", "fee", inactive.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let logs = change_logs(&pool, field.id).await;
    let actions: Vec<_> = logs
        .iter()
        .map(|l| (l.target_type.as_str(), l.action.as_str()))
        .collect();
    assert_eq!(
        actions,
        [
            ("field", "create"),
            ("field", "update"),
            ("field", "deactivate")
        ]
    );
    assert_eq!(logs[1].before_data.as_ref().unwrap()["label"], "費用");
    assert_eq!(logs[1].after_data.as_ref().unwrap()["label"], "維修費");
    assert_eq!(logs[2].before_data.as_ref().unwrap()["is_active"], true);
    assert_eq!(logs[2].after_data.as_ref().unwrap()["is_active"], false);
    // before / after 是完整的定義
    assert_eq!(logs[2].after_data.as_ref().unwrap()["field_key"], "fee");

    // audit_logs 也有對應紀錄，只記變動的屬性
    let audits = audit_logs(&pool, field.id).await;
    let audit_actions: Vec<_> = audits.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(audit_actions, [audit::CREATE, audit::UPDATE, audit::UPDATE]);
    assert!(
        audits
            .iter()
            .all(|a| a.entity_key.as_deref() == Some("field_definition"))
    );
    assert_eq!(audits[1].before_data, Some(json!({ "label": "費用" })));
    assert_eq!(audits[1].after_data, Some(json!({ "label": "維修費" })));
    assert_eq!(audits[2].after_data, Some(json!({ "is_active": false })));
    assert_eq!(
        audits[0].client_info.as_deref(),
        Some("HammerERP-test / 0.1.0")
    );
    let entity_audits = audit_logs(&pool, entity.id).await;
    assert_eq!(entity_audits.len(), 1);
    assert_eq!(
        entity_audits[0].entity_key.as_deref(),
        Some("entity_definition")
    );

    // 修改資料類別
    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/repair_order",
        &token,
        Some(entity_patch(
            &entity,
            json!({ "label": "維修", "icon": "tool" }),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    assert_eq!(detail.entity.label, "維修");
    assert_eq!(detail.entity.version, 2);
    assert_eq!(detail.fields.len(), 1);
    let logs = change_logs(&pool, entity.id).await;
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[1].action, "update");

    // 停用資料類別：不出現在列表，但仍可讀取
    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/repair_order",
        &token,
        Some(entity_patch(&detail.entity, json!({ "is_active": false }))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(change_logs(&pool, entity.id).await[2].action, "deactivate");
    let (_, body) = call(&pool, Method::GET, "/api/v1/entities", &token, None).await;
    assert_eq!(body, json!([]));
    assert!(
        !get_entity(&pool, &token, "repair_order")
            .await
            .entity
            .is_active
    );
}

/// 修改 `field_key` 或 `data_type` 會被拒絕。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn field_key_and_data_type_cannot_change(pool: PgPool) {
    let token = admin(&pool).await;
    create_entity(&pool, &token, "repair_order").await;
    let field = add_text_field(&pool, &token, "repair_order", "title").await;

    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "field_key": "subject" }),
    )
    .await;
    assert_invalid_field(status, &body, "field_key");

    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "data_type": "integer" }),
    )
    .await;
    assert_invalid_field(status, &body, "data_type");

    // 沒有被修改，也沒有留紀錄
    let detail = get_entity(&pool, &token, "repair_order").await;
    assert_eq!(detail.fields, std::slice::from_ref(&field));
    assert_eq!(change_logs(&pool, field.id).await.len(), 1);

    // 帶相同的 field_key 可以
    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "field_key": "title", "label": "主旨" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 資料類別的 key 也不可修改
    let entity = detail.entity;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/repair_order",
        &token,
        Some(entity_patch(&entity, json!({ "key": "other" }))),
    )
    .await;
    assert_invalid_field(status, &body, "key");
}

/// 重複的 `field_key` 回傳 `VALIDATION`，`details.field` 指出欄位。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn duplicate_keys_are_validation_errors(pool: PgPool) {
    let token = admin(&pool).await;
    create_entity(&pool, &token, "repair_order").await;
    let field = add_text_field(&pool, &token, "repair_order", "title").await;

    let (status, body) = add_field(
        &pool,
        &token,
        "repair_order",
        json!({ "field_key": "title", "label": "重複", "data_type": "integer" }),
    )
    .await;
    let err = assert_invalid_field(status, &body, "field_key");
    assert!(err.message.contains("已被使用"), "{body}");

    // 停用的欄位代碼也不可重複使用
    deactivate(&pool, &token, "repair_order", "title", field.version).await;
    let (status, body) = add_field(
        &pool,
        &token,
        "repair_order",
        json!({ "field_key": "title", "label": "重複", "data_type": "text" }),
    )
    .await;
    assert_invalid_field(status, &body, "field_key");

    // 不同資料類別可以使用相同的欄位代碼
    create_entity(&pool, &token, "visit").await;
    add_text_field(&pool, &token, "visit", "title").await;

    // 重複的資料類別代碼
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/entities",
        &token,
        Some(json!({ "key": "repair_order", "label": "重複", "kind": "master" })),
    )
    .await;
    assert_invalid_field(status, &body, "key");
}

/// `version` 不符時回傳 409。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn stale_version_is_conflict(pool: PgPool) {
    let token = admin(&pool).await;
    let entity = create_entity(&pool, &token, "repair_order").await;
    let field = add_text_field(&pool, &token, "repair_order", "title").await;
    let (status, _) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "label": "A" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 用舊的版本號（1）修改
    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "label": "B" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    let (status, body) = deactivate(&pool, &token, "repair_order", "title", field.version).await;
    assert_error(status, &body, ErrorCode::Conflict);
    let (status, body) = call(
        &pool,
        Method::PUT,
        "/api/v1/entities/repair_order/fields/order",
        &token,
        Some(json!({ "fields": [{ "field_key": "title", "version": field.version }] })),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);

    let (status, _) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/repair_order",
        &token,
        Some(entity_patch(&entity, json!({ "label": "A" }))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/repair_order",
        &token,
        Some(entity_patch(&entity, json!({ "label": "B" }))),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);

    let detail = get_entity(&pool, &token, "repair_order").await;
    assert_eq!(detail.entity.label, "A");
    assert_eq!(detail.fields[0].label, "A");
    assert!(detail.fields[0].is_active);
}

/// 系統欄位不可停用（直接在資料庫插入一筆 `is_system = true` 的欄位）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_field_cannot_be_deactivated(pool: PgPool) {
    let token = admin(&pool).await;
    let entity = create_entity(&pool, &token, "repair_order").await;
    let id = insert_system_field(&pool, entity.id, "name").await;
    let field = get_entity(&pool, &token, "repair_order").await.fields[0].clone();
    assert_eq!(field.id, id);
    assert!(field.is_system);

    let (status, body) = deactivate(&pool, &token, "repair_order", "name", field.version).await;
    assert_invalid_field(status, &body, "is_active");
    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "is_active": false }),
    )
    .await;
    assert_invalid_field(status, &body, "is_active");

    // 其他 §7.2 的限制
    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "data_type": "long_text" }),
    )
    .await;
    assert_invalid_field(status, &body, "data_type");
    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "is_unique": true }),
    )
    .await;
    assert_invalid_field(status, &body, "is_unique");

    // 可以修改顯示相關的屬性
    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &field,
        json!({ "label": "客戶名稱", "is_visible": false, "column_width": 200 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(updated.label, "客戶名稱");
    assert!(updated.is_active && !updated.is_visible);
    assert_eq!(change_logs(&pool, id).await.len(), 1);
}

// ---------------------------------------------------------------------------
// 其他行為
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn create_rejects_unsupported_input(pool: PgPool) {
    let token = admin(&pool).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/entities",
        &token,
        Some(json!({ "key": "order", "label": "訂單", "kind": "document" })),
    )
    .await;
    assert_invalid_field(status, &body, "kind");
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/entities",
        &token,
        Some(json!({ "key": "Bad Key", "label": "X", "kind": "master" })),
    )
    .await;
    assert_invalid_field(status, &body, "key");

    create_entity(&pool, &token, "repair_order").await;
    for (data_type, field) in [
        (json!("relation"), "relation_entity_id"),
        (json!("file"), "data_type"),
        (json!("formula"), "data_type"),
        (json!("select"), "options"),
        (json!("decimal"), "decimal_places"),
    ] {
        let (status, body) = add_field(
            &pool,
            &token,
            "repair_order",
            json!({ "field_key": "x", "label": "X", "data_type": data_type }),
        )
        .await;
        assert_invalid_field(status, &body, field);
    }
    // 預設值與型態不符（使用 core 的驗證函式）
    let (status, body) = add_field(
        &pool,
        &token,
        "repair_order",
        json!({
            "field_key": "status",
            "label": "狀態",
            "data_type": "select",
            "options": [{ "value": "open", "label": "處理中" }],
            "default_value": "closed",
        }),
    )
    .await;
    assert_invalid_field(status, &body, "default_value");

    // 不存在的資料類別
    let (status, body) = add_field(
        &pool,
        &token,
        "missing",
        json!({ "field_key": "x", "label": "X", "data_type": "text" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);
    let (status, body) = call(&pool, Method::GET, "/api/v1/entities/missing", &token, None).await;
    assert_error(status, &body, ErrorCode::NotFound);
    assert_eq!(count(&pool, "field_definitions").await, 0);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn reorder_fields_and_detail_order(pool: PgPool) {
    let token = admin(&pool).await;
    create_entity(&pool, &token, "repair_order").await;
    let a = add_text_field(&pool, &token, "repair_order", "a").await;
    let b = add_text_field(&pool, &token, "repair_order", "b").await;
    // 欄位代碼 `order` 與排序路徑同名，仍可修改與停用
    let c = add_text_field(&pool, &token, "repair_order", "order").await;
    let (status, body) = patch_field(
        &pool,
        &token,
        "repair_order",
        &c,
        json!({ "label": "順序" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let c: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(c.label, "順序");
    let (status, body) = deactivate(&pool, &token, "repair_order", "order", c.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let c: FieldDefinition = serde_json::from_value(body).unwrap();

    let order_uri = "/api/v1/entities/repair_order/fields/order";
    // 少列欄位
    let (status, body) = call(
        &pool,
        Method::PUT,
        order_uri,
        &token,
        Some(json!({ "fields": [
            { "field_key": "b", "version": b.version },
            { "field_key": "a", "version": a.version },
        ] })),
    )
    .await;
    assert_invalid_field(status, &body, "fields");

    let (status, body) = call(
        &pool,
        Method::PUT,
        order_uri,
        &token,
        Some(json!({ "fields": [
            { "field_key": "order", "version": c.version },
            { "field_key": "b", "version": b.version },
            { "field_key": "a", "version": a.version },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fields: Vec<FieldDefinition> = serde_json::from_value(body).unwrap();
    let keys: Vec<_> = fields.iter().map(|f| f.field_key.as_str()).collect();
    assert_eq!(keys, ["order", "b", "a"]);
    let orders: Vec<_> = fields.iter().map(|f| f.sort_order).collect();
    assert_eq!(orders, [0, 1, 2]);

    // 詳細資料的欄位依 sort_order 排序
    let detail = get_entity(&pool, &token, "repair_order").await;
    assert_eq!(detail.fields, fields);
    // 原本 sort_order 都是 0：a、b 改變，order 維持 0 不寫入
    assert_eq!(
        change_logs(&pool, a.id).await.last().unwrap().action,
        "update"
    );
    assert_eq!(change_logs(&pool, a.id).await.len(), 2);
    // b 原本是 0、改成 1
    assert_eq!(
        change_logs(&pool, b.id).await[1]
            .after_data
            .as_ref()
            .unwrap()["sort_order"],
        1
    );
}
