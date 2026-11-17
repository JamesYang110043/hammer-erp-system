//! 系統資料類別透過通用資料 API 讀寫專屬資料表的整合測試（#106）：系統欄位存在實體欄位、自訂欄位
//! 存在 `custom_data`，篩選、排序、樂觀鎖定、操作紀錄、權限與自訂資料類別相同
//! （docs/architecture.md §7.1、§7.2、§7.5、§9.4、§10）。
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
use hammer_shared::pagination::Page;
use hammer_shared::record::{LookupItem, Record};
use hammer_shared::schema::FieldDefinition;
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
    err
}

/// 422 VALIDATION，且 `details.fields` 的 `field_key` 依序為 `keys`。
fn assert_invalid_fields(status: StatusCode, body: &Value, keys: &[&str]) {
    let err = assert_error(status, body, ErrorCode::Validation);
    let actual: Vec<String> = err.details.unwrap()["fields"]
        .as_array()
        .unwrap()
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

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

fn record_uri(entity: &str, id: Id) -> String {
    format!("/api/v1/entities/{entity}/records/{id}")
}

async fn create(pool: &PgPool, token: &str, entity: &str, data: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &records_uri(entity),
        token,
        Some(json!({ "data": data })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
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

async fn list_raw(
    pool: &PgPool,
    token: &str,
    entity: &str,
    params: &[(&str, &str)],
) -> (StatusCode, Value) {
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    call(
        pool,
        Method::GET,
        &format!("{}?{}", records_uri(entity), query.join("&")),
        token,
        None,
    )
    .await
}

/// 列表結果中每筆資料的 `code`，依回傳順序。
async fn codes(pool: &PgPool, token: &str, entity: &str, params: &[(&str, &str)]) -> Vec<String> {
    let (status, body) = list_raw(pool, token, entity, params).await;
    assert_eq!(status, StatusCode::OK, "{params:?} {body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    assert_eq!(page.total as usize, page.items.len());
    page.items
        .iter()
        .map(|r| r.data["code"].as_str().unwrap().to_owned())
        .collect()
}

#[derive(Debug, sqlx::FromRow)]
struct UnitRow {
    code: String,
    name: String,
    custom_data: Value,
    version: i32,
    deleted: bool,
}

async fn unit_row(pool: &PgPool, id: Id) -> UnitRow {
    sqlx::query_as(
        "SELECT code, name, custom_data, version, deleted_at IS NOT NULL AS deleted
         FROM units WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    action: String,
    entity_key: Option<String>,
    before_data: Option<Value>,
    after_data: Option<Value>,
}

async fn audit_logs(pool: &PgPool, record_id: Id) -> Vec<AuditRow> {
    sqlx::query_as(
        "SELECT action, entity_key, before_data, after_data
         FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// 系統資料表上某種動態索引的定義；不存在時為 `None`。
async fn index_def(
    pool: &PgPool,
    table: &str,
    kind: IndexKind,
    entity: &str,
    field_key: &str,
) -> Option<String> {
    let name = index_name(kind, entity, field_key).unwrap();
    sqlx::query_scalar("SELECT indexdef FROM pg_indexes WHERE tablename = $1 AND indexname = $2")
        .bind(table)
        .bind(&name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// 新增、修改、刪除、查詢
// ---------------------------------------------------------------------------

/// 單位的完整流程：資料寫在 `units`，沒有單號；版本不符回傳 409；每個寫入都有操作紀錄。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unit_crud_writes_units_table(pool: PgPool) {
    let editor = token_for(&pool, "editor", role::EDITOR).await;

    let created = create(
        &pool,
        &editor,
        "unit",
        json!({ "code": "pcs", "name": "個" }),
    )
    .await;
    assert_eq!(
        created.data,
        *json!({ "code": "pcs", "name": "個" }).as_object().unwrap()
    );
    assert_eq!(created.record_no, None);
    assert_eq!(created.title, "個");
    assert_eq!(created.version, 1);
    assert!(created.formula_errors.is_empty());
    let row = unit_row(&pool, created.id).await;
    assert_eq!((row.code.as_str(), row.name.as_str()), ("pcs", "個"));
    assert_eq!(row.custom_data, json!({}));
    // 沒有寫入 custom_records
    let in_custom: i64 = sqlx::query_scalar("SELECT count(*) FROM custom_records")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(in_custom, 0);

    let (status, body) = call(
        &pool,
        Method::GET,
        &record_uri("unit", created.id),
        &editor,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fetched: Record = serde_json::from_value(body).unwrap();
    assert_eq!(fetched.data, created.data);
    assert_eq!(fetched.lines, Some(Default::default()));

    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("unit", created.id),
        &editor,
        Some(json!({ "version": 1, "data": { "name": "個數" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: Record = serde_json::from_value(body).unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.data["code"], "pcs");
    assert_eq!(updated.data["name"], "個數");
    let row = unit_row(&pool, created.id).await;
    assert_eq!((row.name.as_str(), row.version), ("個數", 2));

    // 沒有變動時不寫入
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("unit", created.id),
        &editor,
        Some(json!({ "version": 2, "data": { "name": "個數" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], 2);

    // 版本不符
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("unit", created.id),
        &editor,
        Some(json!({ "version": 1, "data": { "name": "舊" } })),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("unit", created.id)),
        &editor,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);

    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=2", record_uri("unit", created.id)),
        &editor,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let row = unit_row(&pool, created.id).await;
    assert!(row.deleted);
    assert_eq!(row.version, 3);
    let (status, body) = call(
        &pool,
        Method::GET,
        &record_uri("unit", created.id),
        &editor,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);
    assert!(codes(&pool, &editor, "unit", &[]).await.is_empty());

    let logs = audit_logs(&pool, created.id).await;
    let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
    assert_eq!(actions, [audit::CREATE, audit::UPDATE, audit::DELETE]);
    assert!(logs.iter().all(|l| l.entity_key.as_deref() == Some("unit")));
    assert_eq!(
        logs[0].after_data,
        Some(json!({ "code": "pcs", "name": "個" }))
    );
    assert_eq!(logs[1].before_data, Some(json!({ "name": "個" })));
    assert_eq!(logs[1].after_data, Some(json!({ "name": "個數" })));
    assert_eq!(
        logs[2].before_data,
        Some(json!({ "code": "pcs", "name": "個數" }))
    );
}

/// 系統欄位依欄位定義驗證：必填、`max_length`；`id`、`custom_data` 等不是欄位。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_fields_are_validated(pool: PgPool) {
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("unit"),
        &editor,
        Some(json!({ "data": { "code": "x".repeat(51) } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["code", "name"]);

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("unit"),
        &editor,
        Some(json!({ "data": {
            "code": "pcs", "name": "個", "custom_data": {}, "id": Id::nil(), "version": 1
        } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["custom_data", "id", "version"]);

    // 金額補齊小數位數、整數、布林預設值
    let customer = create(
        &pool,
        &editor,
        "customer",
        json!({ "code": "C001", "name": "甲公司", "credit_limit": "1000", "payment_terms_days": 30 }),
    )
    .await;
    assert_eq!(customer.data["credit_limit"], "1000.00");
    assert_eq!(customer.data["payment_terms_days"], 30);
    assert_eq!(customer.data["is_active"], true);
    assert_eq!(customer.title, "C001 甲公司");
    assert!(customer.data.get("phone").is_none());
    let (limit, phone): (String, Option<String>) =
        sqlx::query_as("SELECT credit_limit::text, phone FROM customers WHERE id = $1")
            .bind(customer.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((limit.as_str(), phone), ("1000.00", None));

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("customer"),
        &editor,
        Some(json!({ "data": { "code": "C002", "name": "乙", "credit_limit": "-1" } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["credit_limit"]);

    // 清空可為空的欄位
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("customer", customer.id),
        &editor,
        Some(json!({ "version": 1, "data": { "credit_limit": null } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"].get("credit_limit").is_none(), "{body}");

    // 系統資料類別的系統欄位代碼不分大小寫不可重複（migration 的唯一索引）
    create(
        &pool,
        &editor,
        "unit",
        json!({ "code": "box", "name": "箱" }),
    )
    .await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("unit"),
        &editor,
        Some(json!({ "data": { "code": "BOX", "name": "大箱" } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["code"]);
}

/// 明細子表（含系統明細子表）不能直接存取。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_detail_entities_are_not_accessible(pool: PgPool) {
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let (status, body) = list_raw(&pool, &editor, "customer_contact", &[]).await;
    assert_error(status, &body, ErrorCode::NotFound);
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("customer_contact"),
        &editor,
        Some(json!({ "data": { "name": "王小明" } })),
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);
}

// ---------------------------------------------------------------------------
// 自訂欄位
// ---------------------------------------------------------------------------

/// 系統資料類別的自訂欄位寫入 `custom_data`；可搜尋、不可重複建立在專屬資料表上。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn custom_fields_are_stored_in_custom_data(pool: PgPool) {
    let admin = token_for(&pool, "admin", role::ADMIN).await;
    let barcode = create_field(
        &pool,
        &admin,
        "unit",
        json!({ "field_key": "ext_code", "label": "外部代碼", "data_type": "text",
                "is_searchable": true, "is_unique": true }),
    )
    .await;
    assert!(barcode.is_unique && barcode.is_searchable);
    let searchable = index_def(&pool, "units", IndexKind::Searchable, "unit", "ext_code")
        .await
        .unwrap();
    assert!(
        searchable.contains("ON public.units") && searchable.contains("custom_data ->> 'ext_code'"),
        "{searchable}"
    );
    let unique = index_def(&pool, "units", IndexKind::Unique, "unit", "ext_code")
        .await
        .unwrap();
    assert!(
        unique.starts_with("CREATE UNIQUE INDEX") && unique.contains("WHERE (deleted_at IS NULL)"),
        "{unique}"
    );
    assert!(!unique.contains("entity_id"), "{unique}");

    let unit = create(
        &pool,
        &admin,
        "unit",
        json!({ "code": "pcs", "name": "個", "ext_code": "EA" }),
    )
    .await;
    assert_eq!(unit.data["ext_code"], "EA");
    let row = unit_row(&pool, unit.id).await;
    assert_eq!(row.custom_data, json!({ "ext_code": "EA" }));

    // 不可重複
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("unit"),
        &admin,
        Some(json!({ "data": { "code": "box", "name": "箱", "ext_code": "EA" } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["ext_code"]);

    // 修改自訂欄位與系統欄位
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("unit", unit.id),
        &admin,
        Some(json!({ "version": 1, "data": { "ext_code": null, "code": "PCS" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = unit_row(&pool, unit.id).await;
    assert_eq!(row.code, "PCS");
    assert_eq!(row.custom_data, json!({}));

    // 停用欄位移除索引
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/entities/unit/fields/ext_code/deactivate",
        &admin,
        Some(json!({ "version": barcode.version })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        index_def(&pool, "units", IndexKind::Unique, "unit", "ext_code").await,
        None
    );
    assert_eq!(
        index_def(&pool, "units", IndexKind::Searchable, "unit", "ext_code").await,
        None
    );
}

/// 既有資料有重複時不能開啟「不可重複」。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unique_custom_field_rejects_existing_duplicates(pool: PgPool) {
    let admin = token_for(&pool, "admin", role::ADMIN).await;
    let grade = create_field(
        &pool,
        &admin,
        "customer",
        json!({ "field_key": "grade", "label": "等級", "data_type": "text" }),
    )
    .await;
    let a = create(
        &pool,
        &admin,
        "customer",
        json!({ "code": "A", "name": "甲", "grade": "VIP" }),
    )
    .await;
    let b = create(
        &pool,
        &admin,
        "customer",
        json!({ "code": "B", "name": "乙", "grade": "VIP" }),
    )
    .await;

    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/customer/fields/grade",
        &admin,
        Some(field_patch(&grade, json!({ "is_unique": true }))),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    let details = err.details.unwrap();
    assert_eq!(details["field"], "is_unique");
    assert_eq!(details["total"], 1);
    let mut ids = vec![a.id, b.id];
    ids.sort();
    assert_eq!(
        details["values"],
        json!([{ "value": "VIP", "count": 2, "record_ids": ids }])
    );
    assert_eq!(
        index_def(&pool, "customers", IndexKind::Unique, "customer", "grade").await,
        None
    );
}

// ---------------------------------------------------------------------------
// 列表、選擇器
// ---------------------------------------------------------------------------

/// 列表的 query string 參數與預期的 `code` 順序。
type ListCase<'a> = (&'a [(&'a str, &'a str)], &'a [&'a str]);

/// 列表可依系統欄位與自訂欄位篩選、排序。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_filters_and_sorts_system_and_custom_fields(pool: PgPool) {
    let admin = token_for(&pool, "admin", role::ADMIN).await;
    create_field(
        &pool,
        &admin,
        "customer",
        json!({ "field_key": "region", "label": "地區", "data_type": "text" }),
    )
    .await;
    for (code, name, limit, active, region) in [
        ("C1", "甲公司", Some("500"), true, Some("北")),
        ("C2", "乙企業", Some("1500"), false, Some("南")),
        ("C3", "丙公司", None, true, None),
    ] {
        let mut data = json!({ "code": code, "name": name, "is_active": active });
        if let Some(limit) = limit {
            data["credit_limit"] = json!(limit);
        }
        if let Some(region) = region {
            data["region"] = json!(region);
        }
        create(&pool, &admin, "customer", data).await;
    }

    let cases: &[ListCase] = &[
        (&[], &["C3", "C2", "C1"]),
        (&[("sort", "code")], &["C1", "C2", "C3"]),
        (&[("sort", "-code")], &["C3", "C2", "C1"]),
        // 數值排序，沒有值的排最後
        (&[("sort", "-credit_limit")], &["C2", "C1", "C3"]),
        (&[("filter.credit_limit.gte", "600")], &["C2"]),
        (&[("filter.credit_limit.empty", "true")], &["C3"]),
        (
            &[("filter.name.contains", "公司"), ("sort", "code")],
            &["C1", "C3"],
        ),
        (&[("filter.code.eq", "C2")], &["C2"]),
        (&[("filter.is_active.eq", "false")], &["C2"]),
        (&[("filter.region.eq", "北")], &["C1"]),
        (&[("filter.region.empty", "true")], &["C3"]),
        (&[("sort", "region")], &["C1", "C2", "C3"]),
        (&[("filter.record_no.eq", "C1")], &[]),
    ];
    for (params, expected) in cases {
        assert_eq!(
            codes(&pool, &admin, "customer", params).await,
            *expected,
            "{params:?}"
        );
    }

    let (status, body) = list_raw(&pool, &admin, "customer", &[("filter.missing.eq", "x")]).await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": "filter.missing.eq" })));

    let (status, body) = list_raw(
        &pool,
        &admin,
        "customer",
        &[("page", "1"), ("page_size", "1"), ("sort", "code")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 3);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
}

/// 選擇器依標題比對，排除停用（`is_active = false`）與已刪除的資料。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn lookup_excludes_inactive_records(pool: PgPool) {
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let active = create(
        &pool,
        &editor,
        "customer",
        json!({ "code": "C1", "name": "甲公司" }),
    )
    .await;
    create(
        &pool,
        &editor,
        "customer",
        json!({ "code": "C2", "name": "乙公司", "is_active": false }),
    )
    .await;
    let deleted = create(
        &pool,
        &editor,
        "customer",
        json!({ "code": "C3", "name": "丙公司" }),
    )
    .await;
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("customer", deleted.id)),
        &editor,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/entities/customer/records/lookup?q=%E5%85%AC%E5%8F%B8",
        &viewer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items: Vec<LookupItem> = serde_json::from_value(body).unwrap();
    assert_eq!(
        items,
        vec![LookupItem {
            id: active.id,
            title: "C1 甲公司".into(),
        }]
    );

    // 單位沒有 is_active，全部列出
    let unit = create(
        &pool,
        &editor,
        "unit",
        json!({ "code": "pcs", "name": "個" }),
    )
    .await;
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/entities/unit/records/lookup",
        &viewer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items: Vec<LookupItem> = serde_json::from_value(body).unwrap();
    assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), [unit.id]);
}

// ---------------------------------------------------------------------------
// 權限
// ---------------------------------------------------------------------------

/// 唯讀者可以查看，不能新增、修改、刪除（伺服器端檢查）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn writes_require_record_write(pool: PgPool) {
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let unit = create(
        &pool,
        &editor,
        "unit",
        json!({ "code": "pcs", "name": "個" }),
    )
    .await;

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("unit"),
        &viewer,
        Some(json!({ "data": { "code": "box", "name": "箱" } })),
    )
    .await;
    assert_error(status, &body, ErrorCode::Forbidden);
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("unit", unit.id),
        &viewer,
        Some(json!({ "version": 1, "data": { "name": "改" } })),
    )
    .await;
    assert_error(status, &body, ErrorCode::Forbidden);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("unit", unit.id)),
        &viewer,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::Forbidden);

    let row = unit_row(&pool, unit.id).await;
    assert_eq!(
        (row.name.as_str(), row.version, row.deleted),
        ("個", 1, false)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM units")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    assert_eq!(codes(&pool, &viewer, "unit", &[]).await, ["pcs"]);
    let (status, body) = call(
        &pool,
        Method::GET,
        &record_uri("unit", unit.id),
        &viewer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
