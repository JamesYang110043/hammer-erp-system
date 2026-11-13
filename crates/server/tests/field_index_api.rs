//! 可搜尋與不可重複欄位的動態索引的整合測試（#50）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::schema::index::{IndexKind, index_name};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
use hammer_shared::schema::{EntityDetail, FieldDefinition};
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const ENTITY: &str = "repair_order";
const OTHER: &str = "service_order";

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

/// 422 VALIDATION，且 `details.fields` 的 `field_key` 依序為 `keys`。
fn assert_invalid_fields(status: StatusCode, body: &Value, keys: &[&str]) {
    let err = assert_error(status, body, ErrorCode::Validation);
    let fields = err.details.unwrap()["fields"].as_array().unwrap().clone();
    let actual: Vec<_> = fields
        .iter()
        .map(|f| f["field_key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(actual, keys, "{body}");
}

async fn admin_token(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    token.into_string()
}

async fn create_entity(pool: &PgPool, token: &str, key: &str) -> Id {
    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        token,
        Some(json!({ "key": key, "label": key, "kind": "master" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    detail.entity.id
}

async fn create_field(pool: &PgPool, token: &str, entity: &str, field: Value) -> FieldDefinition {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        token,
        Some(field),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 建立資料類別與一個文字欄位 `code`。
async fn setup_entity(pool: &PgPool, token: &str, key: &str, field: Value) -> FieldDefinition {
    create_entity(pool, token, key).await;
    let mut body = json!({ "field_key": "code", "label": "編號", "data_type": "text" });
    for (k, v) in field.as_object().unwrap() {
        body[k] = v.clone();
    }
    create_field(pool, token, key, body).await
}

async fn get_field(pool: &PgPool, token: &str, entity: &str, field_key: &str) -> FieldDefinition {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{entity}"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    detail
        .fields
        .into_iter()
        .find(|f| f.field_key == field_key)
        .unwrap()
}

/// `PATCH` 欄位（整筆取代），以 `changes` 覆蓋部分屬性。
async fn patch_field(
    pool: &PgPool,
    token: &str,
    entity: &str,
    field: &FieldDefinition,
    changes: Value,
) -> (StatusCode, Value) {
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
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/entities/{entity}/fields/{}", field.field_key),
        token,
        Some(body),
    )
    .await
}

async fn patch_ok(
    pool: &PgPool,
    token: &str,
    entity: &str,
    field: &FieldDefinition,
    changes: Value,
) -> FieldDefinition {
    let (status, body) = patch_field(pool, token, entity, field, changes).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_record(
    pool: &PgPool,
    token: &str,
    entity: &str,
    data: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/records"),
        token,
        Some(json!({ "data": data })),
    )
    .await
}

async fn create_ok(pool: &PgPool, token: &str, entity: &str, data: Value) -> Record {
    let (status, body) = create_record(pool, token, entity, data).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn soft_delete(pool: &PgPool, token: &str, entity: &str, record: &Record) {
    let (status, body) = call(
        pool,
        Method::DELETE,
        &format!(
            "/api/v1/entities/{entity}/records/{}?version={}",
            record.id, record.version
        ),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// `pg_indexes` 中該欄位某種索引的定義；不存在時為 `None`。
async fn index_def(
    pool: &PgPool,
    kind: IndexKind,
    entity: &str,
    field_key: &str,
) -> Option<String> {
    let name = index_name(kind, entity, field_key).unwrap();
    sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE tablename = 'custom_records' AND indexname = $1",
    )
    .bind(&name)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn schema_change_count(pool: &PgPool, field_id: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM schema_change_logs WHERE target_id = $1")
        .bind(field_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// 開啟「可搜尋」後在 `pg_indexes` 查得到索引，關閉後索引被移除。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn searchable_index_follows_setting(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let field = setup_entity(&pool, &admin, ENTITY, json!({})).await;
    assert_eq!(
        index_def(&pool, IndexKind::Searchable, ENTITY, "code").await,
        None
    );

    let field = patch_ok(
        &pool,
        &admin,
        ENTITY,
        &field,
        json!({ "is_searchable": true }),
    )
    .await;
    let def = index_def(&pool, IndexKind::Searchable, ENTITY, "code")
        .await
        .expect("開啟可搜尋後應建立索引");
    assert!(!def.contains("UNIQUE"), "{def}");
    assert!(def.contains("(data ->> 'code'::text)"), "{def}");
    assert!(def.contains(&field.entity_id.to_string()), "{def}");
    assert!(def.contains("deleted_at IS NULL"), "{def}");
    assert_eq!(
        index_def(&pool, IndexKind::Unique, ENTITY, "code").await,
        None
    );

    patch_ok(
        &pool,
        &admin,
        ENTITY,
        &field,
        json!({ "is_searchable": false }),
    )
    .await;
    assert_eq!(
        index_def(&pool, IndexKind::Searchable, ENTITY, "code").await,
        None
    );

    // 新增欄位時就設定為可搜尋
    let field = create_field(
        &pool,
        &admin,
        ENTITY,
        json!({ "field_key": "title", "label": "標題", "data_type": "text", "is_searchable": true }),
    )
    .await;
    assert!(field.is_searchable);
    assert!(
        index_def(&pool, IndexKind::Searchable, ENTITY, "title")
            .await
            .is_some()
    );
}

/// 開啟「不可重複」後，新增或修改成重複的值回傳 422 並指出欄位。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unique_field_rejects_duplicate_values(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let field = setup_entity(&pool, &admin, ENTITY, json!({})).await;
    create_ok(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
    let b = create_ok(&pool, &admin, ENTITY, json!({ "code": "B" })).await;

    let field = patch_ok(&pool, &admin, ENTITY, &field, json!({ "is_unique": true })).await;
    assert!(field.is_unique);
    let def = index_def(&pool, IndexKind::Unique, ENTITY, "code")
        .await
        .expect("開啟不可重複後應建立唯一索引");
    assert!(def.starts_with("CREATE UNIQUE INDEX"), "{def}");
    assert!(
        def.contains("(entity_id, ((data ->> 'code'::text)))"),
        "{def}"
    );
    assert!(def.contains(&field.entity_id.to_string()), "{def}");
    assert!(def.contains("deleted_at IS NULL"), "{def}");

    let (status, body) = create_record(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
    assert_invalid_fields(status, &body, &["code"]);

    let (status, body) = call(
        &pool,
        Method::PATCH,
        &format!("/api/v1/entities/{ENTITY}/records/{}", b.id),
        &admin,
        Some(json!({ "version": b.version, "data": { "code": "A" } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["code"]);

    // 沒有值的資料不受限制
    create_ok(&pool, &admin, ENTITY, json!({})).await;
    create_ok(&pool, &admin, ENTITY, json!({})).await;

    // 關閉後索引被移除，可以再使用重複的值
    patch_ok(&pool, &admin, ENTITY, &field, json!({ "is_unique": false })).await;
    assert_eq!(
        index_def(&pool, IndexKind::Unique, ENTITY, "code").await,
        None
    );
    create_ok(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
}

/// 既有資料有重複時無法開啟「不可重複」，回應列出衝突的資料；欄位定義與索引都不變。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn existing_duplicates_block_unique(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let field = setup_entity(&pool, &admin, ENTITY, json!({})).await;
    let c1 = create_ok(&pool, &admin, ENTITY, json!({ "code": "C" })).await;
    let a1 = create_ok(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
    create_ok(&pool, &admin, ENTITY, json!({ "code": "B" })).await;
    let a2 = create_ok(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
    let c2 = create_ok(&pool, &admin, ENTITY, json!({ "code": "C" })).await;
    let logs_before = schema_change_count(&pool, field.id).await;

    let (status, body) = patch_field(
        &pool,
        &admin,
        ENTITY,
        &field,
        json!({ "is_unique": true, "is_searchable": true }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(
        err.details,
        Some(json!({
            "field": "is_unique",
            "total": 2,
            "values": [
                { "value": "A", "count": 2, "record_ids": [a1.id, a2.id] },
                { "value": "C", "count": 2, "record_ids": [c1.id, c2.id] },
            ],
        })),
        "{body}"
    );

    // 整個修改都沒有生效（同一個 transaction）
    let current = get_field(&pool, &admin, ENTITY, "code").await;
    assert!(!current.is_unique);
    assert!(!current.is_searchable);
    assert_eq!(current.version, field.version);
    assert_eq!(
        index_def(&pool, IndexKind::Unique, ENTITY, "code").await,
        None
    );
    assert_eq!(
        index_def(&pool, IndexKind::Searchable, ENTITY, "code").await,
        None
    );
    assert_eq!(schema_change_count(&pool, field.id).await, logs_before);

    // 修正重複的資料後可以開啟
    soft_delete(&pool, &admin, ENTITY, &a2).await;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &format!("/api/v1/entities/{ENTITY}/records/{}", c2.id),
        &admin,
        Some(json!({ "version": c2.version, "data": { "code": "D" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    patch_ok(&pool, &admin, ENTITY, &field, json!({ "is_unique": true })).await;
    assert!(
        index_def(&pool, IndexKind::Unique, ENTITY, "code")
            .await
            .is_some()
    );
}

/// 已軟刪除的資料不影響唯一性：開啟時不算重複，開啟後也可以再使用相同的值。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn soft_deleted_records_do_not_affect_uniqueness(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let field = setup_entity(&pool, &admin, ENTITY, json!({})).await;
    let deleted = create_ok(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
    soft_delete(&pool, &admin, ENTITY, &deleted).await;
    let live = create_ok(&pool, &admin, ENTITY, json!({ "code": "A" })).await;

    patch_ok(&pool, &admin, ENTITY, &field, json!({ "is_unique": true })).await;

    let (status, body) = create_record(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
    assert_invalid_fields(status, &body, &["code"]);

    soft_delete(&pool, &admin, ENTITY, &live).await;
    create_ok(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
}

/// 兩個資料類別有相同 `field_key` 時，唯一性互不影響。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn uniqueness_is_scoped_to_entity(pool: PgPool) {
    let admin = admin_token(&pool).await;
    setup_entity(&pool, &admin, ENTITY, json!({ "is_unique": true })).await;
    let other = setup_entity(&pool, &admin, OTHER, json!({})).await;
    let entity_index = index_def(&pool, IndexKind::Unique, ENTITY, "code")
        .await
        .unwrap();

    create_ok(&pool, &admin, ENTITY, json!({ "code": "A" })).await;
    // 另一個資料類別沒有設定不可重複：可以重複，也不受 ENTITY 的索引影響
    create_ok(&pool, &admin, OTHER, json!({ "code": "A" })).await;
    let dup = create_ok(&pool, &admin, OTHER, json!({ "code": "A" })).await;
    soft_delete(&pool, &admin, OTHER, &dup).await;

    // 另一個資料類別也開啟：ENTITY 的 A 不算重複
    patch_ok(&pool, &admin, OTHER, &other, json!({ "is_unique": true })).await;
    let other_index = index_def(&pool, IndexKind::Unique, OTHER, "code")
        .await
        .unwrap();
    assert_ne!(entity_index, other_index);

    let (status, body) = create_record(&pool, &admin, OTHER, json!({ "code": "A" })).await;
    assert_invalid_fields(status, &body, &["code"]);
    create_ok(&pool, &admin, OTHER, json!({ "code": "B" })).await;
    create_ok(&pool, &admin, ENTITY, json!({ "code": "B" })).await;
}

/// 停用欄位時移除索引，重新啟用時再建立。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn deactivation_drops_indexes(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let field = setup_entity(
        &pool,
        &admin,
        ENTITY,
        json!({ "is_unique": true, "is_searchable": true }),
    )
    .await;
    assert!(
        index_def(&pool, IndexKind::Searchable, ENTITY, "code")
            .await
            .is_some()
    );
    assert!(
        index_def(&pool, IndexKind::Unique, ENTITY, "code")
            .await
            .is_some()
    );

    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/entities/{ENTITY}/fields/code/deactivate"),
        &admin,
        Some(json!({ "version": field.version })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let field: FieldDefinition = serde_json::from_value(body).unwrap();
    assert!(!field.is_active);
    assert_eq!(
        index_def(&pool, IndexKind::Searchable, ENTITY, "code").await,
        None
    );
    assert_eq!(
        index_def(&pool, IndexKind::Unique, ENTITY, "code").await,
        None
    );

    patch_ok(&pool, &admin, ENTITY, &field, json!({ "is_active": true })).await;
    assert!(
        index_def(&pool, IndexKind::Searchable, ENTITY, "code")
            .await
            .is_some()
    );
    assert!(
        index_def(&pool, IndexKind::Unique, ENTITY, "code")
            .await
            .is_some()
    );
}

/// 代碼長度達上限時，索引名稱仍在 PostgreSQL 的 63 字元內，且能正常建立。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn long_keys_produce_valid_index_names(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let entity = "e".repeat(30);
    let field_key = "f".repeat(30);
    create_entity(&pool, &admin, &entity).await;
    create_field(
        &pool,
        &admin,
        &entity,
        json!({ "field_key": field_key, "label": "長代碼", "data_type": "text",
                "is_unique": true, "is_searchable": true }),
    )
    .await;
    for kind in [IndexKind::Searchable, IndexKind::Unique] {
        let name = index_name(kind, &entity, &field_key).unwrap();
        assert_eq!(name.len(), 63);
        assert!(
            index_def(&pool, kind, &entity, &field_key).await.is_some(),
            "{name}"
        );
    }
    let mut data = json!({});
    data[field_key.as_str()] = json!("X");
    create_ok(&pool, &admin, &entity, data.clone()).await;
    let (status, body) = create_record(&pool, &admin, &entity, data).await;
    assert_invalid_fields(status, &body, &[field_key.as_str()]);
}
