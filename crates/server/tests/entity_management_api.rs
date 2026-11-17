//! 資料類別管理的 API 整合測試（#73）：單據（`document`）、修改與停用、批次排序、刪除
//! （關聯引用檢查、明細子表一起刪除、動態索引移除、代碼不可再用）、權限與變更紀錄。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::schema::index::{IndexKind, index_name};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
use hammer_shared::schema::{EntityDefinition, EntityDetail, EntityKind};
use hammer_shared::{ApiError, ErrorCode, Id, audit, role};
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
    assert_eq!(err.details.as_ref().unwrap()["field"], field, "{body}");
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

async fn post_entity(pool: &PgPool, token: &str, body: Value) -> (StatusCode, Value) {
    call(pool, Method::POST, "/api/v1/entities", token, Some(body)).await
}

async fn create_entity(pool: &PgPool, token: &str, body: Value) -> EntityDefinition {
    let (status, body) = post_entity(pool, token, body).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value::<EntityDetail>(body).unwrap().entity
}

async fn create_master(pool: &PgPool, token: &str, key: &str) -> EntityDefinition {
    create_entity(
        pool,
        token,
        json!({ "key": key, "label": format!("{key} 名稱"), "kind": "master" }),
    )
    .await
}

async fn create_detail(pool: &PgPool, token: &str, key: &str, parent: Id) -> EntityDefinition {
    create_entity(
        pool,
        token,
        json!({ "key": key, "label": "明細", "kind": "detail", "parent_entity_id": parent }),
    )
    .await
}

async fn add_field(pool: &PgPool, token: &str, entity: &str, body: Value) -> Value {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

async fn get_entity(pool: &PgPool, token: &str, key: &str) -> (StatusCode, Value) {
    call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{key}"),
        token,
        None,
    )
    .await
}

async fn current(pool: &PgPool, token: &str, key: &str) -> EntityDefinition {
    let (status, body) = get_entity(pool, token, key).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value::<EntityDetail>(body).unwrap().entity
}

fn entity_patch(entity: &EntityDefinition, changes: Value) -> Value {
    let mut body = json!({
        "version": entity.version,
        "label": entity.label,
        "icon": entity.icon,
        "numbering_pattern": entity.numbering_pattern,
        "title_template": entity.title_template,
        "sort_order": entity.sort_order,
        "is_active": entity.is_active,
    });
    for (k, v) in changes.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

async fn patch_entity(
    pool: &PgPool,
    token: &str,
    entity: &EntityDefinition,
    changes: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/entities/{}", entity.key),
        token,
        Some(entity_patch(entity, changes)),
    )
    .await
}

async fn delete_entity(pool: &PgPool, token: &str, key: &str, version: i32) -> (StatusCode, Value) {
    call(
        pool,
        Method::DELETE,
        &format!("/api/v1/entities/{key}?version={version}"),
        token,
        None,
    )
    .await
}

async fn reorder(pool: &PgPool, token: &str, items: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::PUT,
        "/api/v1/entities/order",
        token,
        Some(json!({ "entities": items })),
    )
    .await
}

async fn create_record(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/records"),
        token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn index_exists(pool: &PgPool, kind: IndexKind, entity: &str, field_key: &str) -> bool {
    let name = index_name(kind, entity, field_key).unwrap();
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_indexes WHERE tablename = 'custom_records' AND indexname = $1)",
    )
    .bind(&name)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct ChangeLog {
    action: String,
    before_data: Option<Value>,
    after_data: Option<Value>,
}

async fn change_logs(pool: &PgPool, target_id: Id) -> Vec<ChangeLog> {
    sqlx::query_as(
        "SELECT action, before_data, after_data
         FROM schema_change_logs
         WHERE target_type = 'entity' AND target_id = $1 ORDER BY changed_at, id",
    )
    .bind(target_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn audit_actions(pool: &PgPool, record_id: Id) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT action FROM audit_logs
         WHERE entity_key = 'entity_definition' AND record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn schema_change_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM schema_change_logs")
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// 單據
// ---------------------------------------------------------------------------

/// 建立 `document` 並設定單號規則，新增資料時依規則產生單號；列表回傳 `kind`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn document_entity_with_numbering_pattern(pool: PgPool) {
    let token = admin(&pool).await;
    let doc = create_entity(
        &pool,
        &token,
        json!({
            "key": "repair_order", "label": "維修單", "icon": "tool", "kind": "document",
            "numbering_pattern": "RO{yyyy}-{000}",
        }),
    )
    .await;
    assert_eq!(doc.kind, EntityKind::Document);
    assert_eq!(doc.numbering_pattern.as_deref(), Some("RO{yyyy}-{000}"));
    assert_eq!(doc.parent_entity_id, None);

    let record = create_record(&pool, &token, "repair_order", json!({ "data": {} })).await;
    let no = record.record_no.unwrap();
    assert!(no.starts_with("RO") && no.ends_with("-001"), "{no}");

    // 單據也可以有明細子表
    let detail = create_detail(&pool, &token, "repair_parts", doc.id).await;
    assert_eq!(detail.parent_entity_id, Some(doc.id));

    let (status, body) = call(&pool, Method::GET, "/api/v1/entities", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list: Vec<EntityDefinition> = serde_json::from_value(body).unwrap();
    let found = list.iter().find(|e| e.key == "repair_order").unwrap();
    assert_eq!(found.kind, EntityKind::Document);

    // 單據不可指定主表
    let (status, body) = post_entity(
        &pool,
        &token,
        json!({ "key": "bad_doc", "label": "x", "kind": "document", "parent_entity_id": doc.id }),
    )
    .await;
    assert_invalid_field(status, &body, "parent_entity_id");
}

// ---------------------------------------------------------------------------
// 修改、停用、排序
// ---------------------------------------------------------------------------

/// 修改名稱、圖示、排序；停用後不出現在列表、資料 API 回傳 404；重新啟用後恢復。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn update_deactivate_and_reactivate(pool: PgPool) {
    let token = admin(&pool).await;
    let entity = create_master(&pool, &token, "repair_order").await;

    let (status, body) = patch_entity(
        &pool,
        &token,
        &entity,
        json!({ "label": "維修工單", "icon": "wrench", "sort_order": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: EntityDetail = serde_json::from_value(body).unwrap();
    assert_eq!(updated.entity.label, "維修工單");
    assert_eq!(updated.entity.icon.as_deref(), Some("wrench"));
    assert_eq!(updated.entity.sort_order, 7);
    assert_eq!(updated.entity.version, entity.version + 1);

    let (status, body) = patch_entity(
        &pool,
        &token,
        &updated.entity,
        json!({ "is_active": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = call(&pool, Method::GET, "/api/v1/entities", &token, None).await;
    assert!(
        !body
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["key"] == "repair_order")
    );
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/entities/repair_order/records",
        &token,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);

    let inactive = current(&pool, &token, "repair_order").await;
    assert!(!inactive.is_active);
    let (status, body) = patch_entity(&pool, &token, &inactive, json!({ "is_active": true })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    create_record(&pool, &token, "repair_order", json!({ "data": {} })).await;
}

/// `include_inactive=true` 列出已停用的資料類別（管理畫面用），不列出已刪除的；
/// 沒有 `schema.manage` 時回傳 403。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_entities_including_inactive(pool: PgPool) {
    let token = admin(&pool).await;
    let active = create_master(&pool, &token, "active_one").await;
    let inactive = create_master(&pool, &token, "inactive_one").await;
    let deleted = create_master(&pool, &token, "deleted_one").await;
    let (status, body) =
        patch_entity(&pool, &token, &inactive, json!({ "is_active": false })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = delete_entity(&pool, &token, "deleted_one", deleted.version).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let keys = |body: &Value| -> Vec<String> {
        body.as_array()
            .unwrap()
            .iter()
            .filter(|e| e["is_system"] == false)
            .map(|e| e["key"].as_str().unwrap().to_owned())
            .collect()
    };
    for uri in [
        "/api/v1/entities",
        "/api/v1/entities?include_inactive=false",
    ] {
        let (status, body) = call(&pool, Method::GET, uri, &token, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(keys(&body), [active.key.as_str()], "{uri}");
    }
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/entities?include_inactive=true",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let all: Vec<EntityDefinition> = serde_json::from_value(body).unwrap();
    let listed: Vec<(&str, bool)> = all
        .iter()
        .filter(|e| !e.is_system)
        .map(|e| (e.key.as_str(), e.is_active))
        .collect();
    assert_eq!(listed, [("active_one", true), ("inactive_one", false)]);

    for (name, role) in [("editor", role::EDITOR), ("viewer", role::VIEWER)] {
        let other = token_for(&pool, name, role).await;
        let (status, body) = call(&pool, Method::GET, "/api/v1/entities", &other, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = call(
            &pool,
            Method::GET,
            "/api/v1/entities?include_inactive=true",
            &other,
            None,
        )
        .await;
        let err = assert_error(status, &body, ErrorCode::Forbidden);
        assert_eq!(err.details, Some(json!({ "permission": "schema.manage" })));
    }
}

/// 系統資料類別不可停用、不可刪除，但可以調整排序。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_entity_cannot_be_deactivated_or_deleted(pool: PgPool) {
    let token = admin(&pool).await;
    // migration 寫入的系統資料類別（§8.6）
    let system = current(&pool, &token, "customer").await;

    let (status, body) = patch_entity(&pool, &token, &system, json!({ "is_active": false })).await;
    assert_invalid_field(status, &body, "is_active");
    let (status, body) = delete_entity(&pool, &token, "customer", system.version).await;
    assert_invalid_field(status, &body, "is_system");

    let (status, body) = reorder(
        &pool,
        &token,
        json!([{ "key": "customer", "version": system.version, "sort_order": 3 }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = current(&pool, &token, "customer").await;
    assert!(after.is_active);
    assert_eq!(after.sort_order, 3);
}

/// 一次調整多筆排序：只列出要調整的資料類別；代碼錯誤 422；版本不符 409 且整批不寫入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn reorder_entities_in_one_request(pool: PgPool) {
    let token = admin(&pool).await;
    let a = create_master(&pool, &token, "alpha").await;
    let b = create_master(&pool, &token, "beta").await;
    let c = create_master(&pool, &token, "gamma").await;

    let (status, body) = reorder(
        &pool,
        &token,
        json!([
            { "key": "gamma", "version": c.version, "sort_order": 1 },
            { "key": "alpha", "version": a.version, "sort_order": 2 },
            { "key": "beta", "version": b.version, "sort_order": 0 },
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list: Vec<EntityDefinition> = serde_json::from_value(body).unwrap();
    let keys: Vec<&str> = list
        .iter()
        .filter(|e| !e.is_system)
        .map(|e| e.key.as_str())
        .collect();
    // 依 sort_order、id 排序
    assert_eq!(keys, ["beta", "gamma", "alpha"]);
    let by_key = |k: &str| list.iter().find(|e| e.key == k).unwrap().clone();
    // 排序沒變的（beta 原本就是 0）不更新
    assert_eq!(by_key("beta").version, b.version);
    assert_eq!(by_key("alpha").version, a.version + 1);
    assert_eq!(by_key("gamma").version, c.version + 1);

    for items in [
        json!([{ "key": "missing", "version": 1, "sort_order": 0 }]),
        json!([
            { "key": "beta", "version": b.version, "sort_order": 1 },
            { "key": "beta", "version": b.version, "sort_order": 2 },
        ]),
    ] {
        let (status, body) = reorder(&pool, &token, items).await;
        assert_invalid_field(status, &body, "entities");
    }

    // alpha 的版本已過期：整批不寫入，beta 也不變
    let before = schema_change_count(&pool).await;
    let (status, body) = reorder(
        &pool,
        &token,
        json!([
            { "key": "beta", "version": b.version, "sort_order": 9 },
            { "key": "alpha", "version": a.version, "sort_order": 9 },
        ]),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    assert_eq!(current(&pool, &token, "beta").await.sort_order, 0);
    assert_eq!(schema_change_count(&pool).await, before);
}

/// 代碼為 `order` 的資料類別仍可透過 `/entities/order` 查詢、修改、刪除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn entity_named_order_is_still_reachable(pool: PgPool) {
    let token = admin(&pool).await;
    let entity = create_master(&pool, &token, "order").await;
    let fetched = current(&pool, &token, "order").await;
    assert_eq!(fetched.id, entity.id);

    let (status, body) = patch_entity(&pool, &token, &fetched, json!({ "label": "訂單" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated = current(&pool, &token, "order").await;
    assert_eq!(updated.label, "訂單");

    let (status, body) = delete_entity(&pool, &token, "order", updated.version).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = get_entity(&pool, &token, "order").await;
    assert_error(status, &body, ErrorCode::NotFound);
}

// ---------------------------------------------------------------------------
// 刪除
// ---------------------------------------------------------------------------

/// 被其他有效資料類別的有效關聯欄位引用時，刪除回傳 422，`details.fields` 列出這些欄位。
/// 自己引用自己、已停用的關聯欄位不影響刪除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn delete_rejected_while_referenced_by_relation(pool: PgPool) {
    let token = admin(&pool).await;
    let customer = create_master(&pool, &token, "client").await;
    // 自己引用自己
    add_field(
        &pool,
        &token,
        "client",
        json!({ "field_key": "referrer", "label": "介紹人", "data_type": "relation",
                "relation_entity_id": customer.id }),
    )
    .await;
    create_master(&pool, &token, "repair_order").await;
    let relation = add_field(
        &pool,
        &token,
        "repair_order",
        json!({ "field_key": "customer", "label": "客戶", "data_type": "relation",
                "relation_entity_id": customer.id }),
    )
    .await;
    create_master(&pool, &token, "visit").await;
    add_field(
        &pool,
        &token,
        "visit",
        json!({ "field_key": "who", "label": "拜訪對象", "data_type": "relation",
                "relation_entity_id": customer.id }),
    )
    .await;

    let customer = current(&pool, &token, "client").await;
    let (status, body) = delete_entity(&pool, &token, "client", customer.version).await;
    let err = assert_invalid_field(status, &body, "relation_entity_id");
    assert_eq!(
        err.details.unwrap()["fields"],
        json!([
            { "entity_key": "repair_order", "entity_label": "repair_order 名稱",
              "field_key": "customer", "field_label": "客戶" },
            { "entity_key": "visit", "entity_label": "visit 名稱",
              "field_key": "who", "field_label": "拜訪對象" },
        ])
    );
    assert!(get_entity(&pool, &token, "client").await.0.is_success());

    // 停用引用的欄位、停用引用的資料類別後就可以刪除
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/entities/repair_order/fields/customer/deactivate",
        &token,
        Some(json!({ "version": relation["version"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let visit = current(&pool, &token, "visit").await;
    let (status, body) = patch_entity(&pool, &token, &visit, json!({ "is_active": false })).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = delete_entity(&pool, &token, "client", customer.version).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // 刪除後不能再建立指向它的關聯欄位
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/entities/repair_order/fields",
        &token,
        Some(
            json!({ "field_key": "customer2", "label": "客戶", "data_type": "relation",
                     "relation_entity_id": customer.id }),
        ),
    )
    .await;
    assert_invalid_field(status, &body, "relation_entity_id");
}

/// 刪除主表時明細子表一起刪除、動態索引被移除；資料保留；版本不符 409。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn delete_removes_details_and_dynamic_indexes(pool: PgPool) {
    let token = admin(&pool).await;
    let master = create_master(&pool, &token, "repair_order").await;
    add_field(
        &pool,
        &token,
        "repair_order",
        json!({ "field_key": "code", "label": "代碼", "data_type": "text",
                "is_searchable": true, "is_unique": true }),
    )
    .await;
    let detail = create_detail(&pool, &token, "repair_parts", master.id).await;
    add_field(
        &pool,
        &token,
        "repair_parts",
        json!({ "field_key": "part_no", "label": "料號", "data_type": "text",
                "is_searchable": true, "is_unique": true }),
    )
    .await;
    // 另一個主表的明細子表不受影響
    let other = create_master(&pool, &token, "other").await;
    let other_detail = create_detail(&pool, &token, "other_lines", other.id).await;

    let record = create_record(
        &pool,
        &token,
        "repair_order",
        json!({ "data": { "code": "A1" },
                "lines": { "repair_parts": [{ "data": { "part_no": "P1" } }] } }),
    )
    .await;

    for (entity, field) in [("repair_order", "code"), ("repair_parts", "part_no")] {
        assert!(index_exists(&pool, IndexKind::Searchable, entity, field).await);
        assert!(index_exists(&pool, IndexKind::Unique, entity, field).await);
    }

    let master = current(&pool, &token, "repair_order").await;
    let (status, body) = delete_entity(&pool, &token, "repair_order", master.version - 1).await;
    assert_error(status, &body, ErrorCode::Conflict);

    let (status, body) = delete_entity(&pool, &token, "repair_order", master.version).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    for key in ["repair_order", "repair_parts"] {
        let (status, body) = get_entity(&pool, &token, key).await;
        assert_error(status, &body, ErrorCode::NotFound);
    }
    for (entity, field) in [("repair_order", "code"), ("repair_parts", "part_no")] {
        assert!(!index_exists(&pool, IndexKind::Searchable, entity, field).await);
        assert!(!index_exists(&pool, IndexKind::Unique, entity, field).await);
    }
    let deleted: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM entity_definitions WHERE deleted_at IS NOT NULL ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(deleted, [master.id, detail.id]);
    assert_eq!(
        current(&pool, &token, "other_lines").await.id,
        other_detail.id
    );

    // 資料保留，沒有被軟刪除
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM custom_records
         WHERE (id = $1 OR parent_record_id = $1) AND deleted_at IS NULL",
    )
    .bind(record.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 2);

    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/repair_order/records/{}", record.id),
        &token,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);

    // 已刪除的資料類別不能再刪除
    let (status, body) = delete_entity(&pool, &token, "repair_order", master.version + 1).await;
    assert_error(status, &body, ErrorCode::NotFound);
}

/// 只刪除明細子表時，主表不受影響，回應的 `lines` 不再包含它。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn delete_detail_entity_only(pool: PgPool) {
    let token = admin(&pool).await;
    let master = create_master(&pool, &token, "repair_order").await;
    let detail = create_detail(&pool, &token, "repair_parts", master.id).await;
    let record = create_record(
        &pool,
        &token,
        "repair_order",
        json!({ "data": {}, "lines": { "repair_parts": [{ "data": {} }] } }),
    )
    .await;
    assert!(record.lines.unwrap().contains_key("repair_parts"));

    let (status, body) = delete_entity(&pool, &token, "repair_parts", detail.version).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let after = current(&pool, &token, "repair_order").await;
    assert_eq!(after.version, master.version);
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/repair_order/records/{}", record.id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["lines"], json!({}));
}

/// 已刪除的資料類別代碼不可再建立（含明細子表）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn deleted_key_cannot_be_reused(pool: PgPool) {
    let token = admin(&pool).await;
    let master = create_master(&pool, &token, "repair_order").await;
    create_detail(&pool, &token, "repair_parts", master.id).await;
    let (status, body) = delete_entity(&pool, &token, "repair_order", master.version).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = post_entity(
        &pool,
        &token,
        json!({ "key": "repair_order", "label": "維修單", "kind": "document" }),
    )
    .await;
    assert_invalid_field(status, &body, "key");
    let other = create_master(&pool, &token, "other").await;
    let (status, body) = post_entity(
        &pool,
        &token,
        json!({ "key": "repair_parts", "label": "明細", "kind": "detail",
                "parent_entity_id": other.id }),
    )
    .await;
    assert_invalid_field(status, &body, "key");
}

// ---------------------------------------------------------------------------
// 權限與紀錄
// ---------------------------------------------------------------------------

/// editor、viewer 修改、停用、排序、刪除資料類別、建立單據都回傳 403，且沒有寫入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn editor_and_viewer_cannot_manage_entities(pool: PgPool) {
    let admin = admin(&pool).await;
    let entity = create_master(&pool, &admin, "repair_order").await;
    let before = schema_change_count(&pool).await;

    for (name, role) in [("editor", role::EDITOR), ("viewer", role::VIEWER)] {
        let token = token_for(&pool, name, role).await;
        let responses = [
            patch_entity(&pool, &token, &entity, json!({ "label": "改名" })).await,
            patch_entity(&pool, &token, &entity, json!({ "is_active": false })).await,
            reorder(
                &pool,
                &token,
                json!([{ "key": "repair_order", "version": entity.version, "sort_order": 5 }]),
            )
            .await,
            delete_entity(&pool, &token, "repair_order", entity.version).await,
            post_entity(
                &pool,
                &token,
                json!({ "key": "doc", "label": "單據", "kind": "document" }),
            )
            .await,
        ];
        for (status, body) in responses {
            let err = assert_error(status, &body, ErrorCode::Forbidden);
            assert_eq!(err.details, Some(json!({ "permission": "schema.manage" })));
        }
    }
    assert_eq!(schema_change_count(&pool).await, before);
    let after = current(&pool, &admin, "repair_order").await;
    assert_eq!(after.version, entity.version);
}

/// 建立、修改、停用、重新啟用、排序、刪除都寫入 `schema_change_logs` 與 `audit_logs`；
/// 沒有變動時不寫入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn every_entity_change_writes_schema_change_logs(pool: PgPool) {
    let token = admin(&pool).await;
    let doc = create_entity(
        &pool,
        &token,
        json!({ "key": "repair_order", "label": "維修單", "kind": "document" }),
    )
    .await;
    let detail = create_detail(&pool, &token, "repair_parts", doc.id).await;

    let (status, _) = patch_entity(&pool, &token, &doc, json!({ "label": "維修工單" })).await;
    assert_eq!(status, StatusCode::OK);
    let e = current(&pool, &token, "repair_order").await;
    // 沒有變動：不寫入
    let (status, _) = patch_entity(&pool, &token, &e, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = patch_entity(&pool, &token, &e, json!({ "is_active": false })).await;
    assert_eq!(status, StatusCode::OK);
    let e = current(&pool, &token, "repair_order").await;
    let (status, _) = patch_entity(&pool, &token, &e, json!({ "is_active": true })).await;
    assert_eq!(status, StatusCode::OK);
    let e = current(&pool, &token, "repair_order").await;
    let (status, _) = reorder(
        &pool,
        &token,
        json!([{ "key": "repair_order", "version": e.version, "sort_order": 4 }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let e = current(&pool, &token, "repair_order").await;
    // 排序沒變：不寫入
    let (status, _) = reorder(
        &pool,
        &token,
        json!([{ "key": "repair_order", "version": e.version, "sort_order": 4 }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = delete_entity(&pool, &token, "repair_order", e.version).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let logs = change_logs(&pool, doc.id).await;
    let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
    assert_eq!(
        actions,
        [
            "create",
            "update",
            "deactivate",
            "update",
            "update",
            "delete"
        ]
    );
    assert_eq!(logs[0].before_data, None);
    assert_eq!(logs[0].after_data.as_ref().unwrap()["kind"], "document");
    assert_eq!(logs[1].after_data.as_ref().unwrap()["label"], "維修工單");
    assert_eq!(logs[2].after_data.as_ref().unwrap()["is_active"], false);
    assert_eq!(logs[3].after_data.as_ref().unwrap()["is_active"], true);
    assert_eq!(logs[4].before_data.as_ref().unwrap()["sort_order"], 0);
    assert_eq!(logs[4].after_data.as_ref().unwrap()["sort_order"], 4);
    assert_eq!(logs[5].before_data.as_ref().unwrap()["key"], "repair_order");
    assert_eq!(logs[5].after_data, None);

    // 明細子表一起刪除，也有紀錄
    let detail_actions: Vec<String> = change_logs(&pool, detail.id)
        .await
        .into_iter()
        .map(|l| l.action)
        .collect();
    assert_eq!(detail_actions, ["create", "delete"]);

    assert_eq!(
        audit_actions(&pool, doc.id).await,
        [
            audit::CREATE,
            audit::UPDATE,
            audit::UPDATE,
            audit::UPDATE,
            audit::UPDATE,
            audit::DELETE
        ]
    );
    assert_eq!(
        audit_actions(&pool, detail.id).await,
        [audit::CREATE, audit::DELETE]
    );
}
