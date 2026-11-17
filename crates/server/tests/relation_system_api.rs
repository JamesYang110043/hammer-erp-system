//! 關聯欄位指向系統資料類別的 API 整合測試（#110，docs/architecture.md §7.3、§7.6）。
//!
//! - 自訂資料類別可以新增指向 `customer` 的關聯欄位，寫入、讀取（`relations` 的標題依客戶的標題規則）、
//!   依 `eq` 篩選
//! - 指向不存在或已刪除的客戶 → `422`；客戶之後被刪除時保留原值並標示已刪除
//! - 公式的關聯引用可以讀取客戶的系統欄位與自訂欄位
//! - 不可指向系統明細子表（`customer_contact`）
//! - 系統資料表的外鍵（商品的 `unit_id`）也產生關聯目標的標題
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::pagination::Page;
use hammer_shared::record::Record;
use hammer_shared::{ApiError, ErrorCode, Id, new_id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// 有關聯欄位的自訂資料類別（維修單）。
const REPAIR: &str = "repair_order";

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

fn assert_validation(status: StatusCode, body: &Value) -> ApiError {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, ErrorCode::Validation, "{body}");
    err
}

/// 422 VALIDATION，且 `details.field` 為 `field`。
fn assert_invalid(status: StatusCode, body: &Value, field: &str) {
    let err = assert_validation(status, body);
    assert_eq!(err.details.unwrap()["field"], json!(field), "{body}");
}

/// 422 VALIDATION，且 `details.fields` 的 `field_key` 依序為 `keys`。
fn assert_invalid_fields(status: StatusCode, body: &Value, keys: &[&str]) {
    let err = assert_validation(status, body);
    let fields = err.details.unwrap()["fields"].as_array().unwrap().clone();
    let actual: Vec<_> = fields
        .iter()
        .map(|f| f["field_key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(actual, keys, "{body}");
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

async fn entity_id(pool: &PgPool, key: &str) -> Id {
    sqlx::query_scalar("SELECT id FROM entity_definitions WHERE key = $1")
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap()
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

async fn create_field(pool: &PgPool, admin: &str, entity: &str, field: Value) {
    let (status, body) = create_field_raw(pool, admin, entity, field).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

fn relation_field(key: &str, target: Id) -> Value {
    json!({
        "field_key": key,
        "label": key,
        "data_type": "relation",
        "relation_entity_id": target,
    })
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

async fn list(pool: &PgPool, token: &str, entity: &str, query: &str) -> Vec<Record> {
    let uri = format!("{}?{query}", records_uri(entity));
    let (status, body) = call(pool, Method::GET, &uri, token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    page.items
}

/// 建立自訂資料類別 [`REPAIR`]（`note` text、`customer` relation → `customer`），
/// 回傳 `(管理者 token, 一般使用者 token)`。
async fn setup(pool: &PgPool) -> (String, String) {
    let admin = token_for(pool, "admin", role::ADMIN).await;
    let editor = token_for(pool, "editor", role::EDITOR).await;
    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        &admin,
        Some(json!({ "key": REPAIR, "label": "維修單", "kind": "master" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    create_field(
        pool,
        &admin,
        REPAIR,
        json!({ "field_key": "note", "label": "備註", "data_type": "text" }),
    )
    .await;
    let customer = entity_id(pool, "customer").await;
    create_field(pool, &admin, REPAIR, relation_field("customer", customer)).await;
    (admin, editor)
}

async fn customer(pool: &PgPool, token: &str, code: &str, name: &str, data: Value) -> Record {
    let mut body = json!({ "code": code, "name": name });
    body.as_object_mut()
        .unwrap()
        .extend(data.as_object().unwrap().clone());
    create(pool, token, "customer", body).await
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn relation_to_customer_writes_reads_and_filters(pool: PgPool) {
    let (_, editor) = setup(&pool).await;
    let a = customer(&pool, &editor, "C001", "甲公司", json!({})).await;
    let b = customer(&pool, &editor, "C002", "乙公司", json!({})).await;

    let r1 = create(&pool, &editor, REPAIR, json!({ "customer": a.id })).await;
    assert_eq!(r1.data["customer"], json!(a.id.to_string()));
    let r = &r1.relations["customer"];
    // 標題依客戶的標題範本 `{code} {name}`
    assert_eq!(
        (r.id, r.title.as_str(), r.deleted),
        (a.id, "C001 甲公司", false)
    );
    let r2 = create(&pool, &editor, REPAIR, json!({ "customer": b.id })).await;
    create(&pool, &editor, REPAIR, json!({ "note": "沒有客戶" })).await;
    let r4 = create(&pool, &editor, REPAIR, json!({ "customer": a.id })).await;

    // 單筆讀取
    let got = get(&pool, &editor, REPAIR, r2.id).await;
    assert_eq!(got.relations["customer"].title, "C002 乙公司");

    // 依 eq 篩選；列表也有關聯資訊
    let items = list(
        &pool,
        &editor,
        REPAIR,
        &format!("filter.customer.eq={}", a.id),
    )
    .await;
    assert_eq!(
        items.iter().map(|i| i.id).collect::<Vec<_>>(),
        [r4.id, r1.id]
    );
    assert!(
        items
            .iter()
            .all(|i| i.relations["customer"].title == "C001 甲公司")
    );

    // 修改關聯
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(REPAIR, r1.id),
        &editor,
        Some(json!({ "version": r1.version, "data": { "customer": b.id } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: Record = serde_json::from_value(body).unwrap();
    assert_eq!(updated.relations["customer"].id, b.id);

    // 停用的客戶仍可寫入（§7.1）
    let inactive = customer(
        &pool,
        &editor,
        "C003",
        "丙公司",
        json!({ "is_active": false }),
    )
    .await;
    create(&pool, &editor, REPAIR, json!({ "customer": inactive.id })).await;
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn relation_to_missing_or_deleted_customer_is_rejected(pool: PgPool) {
    let (_, editor) = setup(&pool).await;
    let deleted = customer(&pool, &editor, "C001", "甲公司", json!({})).await;
    delete(&pool, &editor, "customer", &deleted).await;
    // 其他系統資料類別的 id
    let unit = create(
        &pool,
        &editor,
        "unit",
        json!({ "code": "pcs", "name": "個" }),
    )
    .await;

    for id in [new_id(), deleted.id, unit.id] {
        let (status, body) = create_raw(&pool, &editor, REPAIR, json!({ "customer": id })).await;
        assert_invalid_fields(status, &body, &["customer"]);
    }

    // 客戶之後被刪除：保留原值，標示已刪除，標題為刪除前的標題
    let alive = customer(&pool, &editor, "C002", "乙公司", json!({})).await;
    let repair = create(&pool, &editor, REPAIR, json!({ "customer": alive.id })).await;
    for id in [new_id(), deleted.id] {
        let (status, body) = call(
            &pool,
            Method::PATCH,
            &record_uri(REPAIR, repair.id),
            &editor,
            Some(json!({ "version": repair.version, "data": { "customer": id } })),
        )
        .await;
        assert_invalid_fields(status, &body, &["customer"]);
    }
    let alive = get(&pool, &editor, "customer", alive.id).await;
    delete(&pool, &editor, "customer", &alive).await;
    let after = get(&pool, &editor, REPAIR, repair.id).await;
    assert_eq!(after.data["customer"], json!(alive.id.to_string()));
    let r = &after.relations["customer"];
    assert_eq!((r.title.as_str(), r.deleted), ("C002 乙公司", true));

    // 原樣送回已刪除的關聯仍可儲存其他欄位
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(REPAIR, repair.id),
        &editor,
        Some(json!({
            "version": after.version,
            "data": { "note": "改過", "customer": alive.id },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn formula_reads_customer_system_and_custom_fields(pool: PgPool) {
    let (admin, editor) = setup(&pool).await;
    // 客戶的自訂欄位
    create_field(
        &pool,
        &admin,
        "customer",
        json!({ "field_key": "grade", "label": "等級", "data_type": "text" }),
    )
    .await;
    create_field(
        &pool,
        &admin,
        REPAIR,
        json!({
            "field_key": "due_days",
            "label": "付款天數 + 7",
            "data_type": "formula",
            "formula": "{customer.payment_terms_days} + 7",
            "formula_result_type": "integer",
        }),
    )
    .await;
    create_field(
        &pool,
        &admin,
        REPAIR,
        json!({
            "field_key": "label",
            "label": "客戶等級",
            "data_type": "formula",
            "formula": "{customer.name} & \"/\" & {customer.grade}",
            "formula_result_type": "text",
        }),
    )
    .await;

    let c = customer(
        &pool,
        &editor,
        "C001",
        "甲公司",
        json!({ "payment_terms_days": 30, "grade": "A" }),
    )
    .await;
    let repair = create(&pool, &editor, REPAIR, json!({ "customer": c.id })).await;
    assert!(
        repair.formula_errors.is_empty(),
        "{:?}",
        repair.formula_errors
    );
    assert_eq!(repair.data["due_days"], json!(37));
    assert_eq!(repair.data["label"], json!("甲公司/A"));

    // 客戶修改後不會自動更新；再次存檔時讀取目前的值
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("customer", c.id),
        &editor,
        Some(json!({ "version": c.version, "data": { "payment_terms_days": 60 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        get(&pool, &editor, REPAIR, repair.id).await.data["due_days"],
        json!(37)
    );
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(REPAIR, repair.id),
        &editor,
        Some(json!({ "version": repair.version, "data": { "note": "再存一次" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let saved: Record = serde_json::from_value(body).unwrap();
    assert_eq!(saved.data["due_days"], json!(67));

    // 不存在的系統欄位
    let (status, body) = create_field_raw(
        &pool,
        &admin,
        REPAIR,
        json!({
            "field_key": "bad",
            "label": "bad",
            "data_type": "formula",
            "formula": "{customer.no_such_field}",
            "formula_result_type": "text",
        }),
    )
    .await;
    assert_validation(status, &body);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn relation_cannot_target_system_detail(pool: PgPool) {
    let (admin, _) = setup(&pool).await;
    for key in ["customer_contact", "supplier_contact"] {
        let target = entity_id(&pool, key).await;
        let (status, body) =
            create_field_raw(&pool, &admin, REPAIR, relation_field("contact", target)).await;
        assert_invalid(status, &body, "relation_entity_id");
    }
    // 其他系統主檔可以
    for key in ["supplier", "product", "unit", "product_category"] {
        let target = entity_id(&pool, key).await;
        create_field(&pool, &admin, REPAIR, relation_field(key, target)).await;
    }
    // 系統資料類別也可以新增指向系統資料類別的自訂關聯欄位
    let customer = entity_id(&pool, "customer").await;
    create_field(
        &pool,
        &admin,
        "supplier",
        relation_field("customer", customer),
    )
    .await;
}

/// 系統資料表的外鍵（商品的 `unit_id`）也是指向系統資料類別的關聯欄位，回應有目標的標題。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_foreign_key_relations_have_titles(pool: PgPool) {
    let (_, editor) = setup(&pool).await;
    let unit = create(
        &pool,
        &editor,
        "unit",
        json!({ "code": "pcs", "name": "個" }),
    )
    .await;
    let product = create(
        &pool,
        &editor,
        "product",
        json!({ "code": "P001", "name": "螺絲", "unit_id": unit.id }),
    )
    .await;
    let r = &product.relations["unit_id"];
    assert_eq!((r.id, r.title.as_str(), r.deleted), (unit.id, "個", false));
    let items = list(&pool, &editor, "product", "").await;
    assert_eq!(items[0].relations["unit_id"].title, "個");
}
