//! 客戶與廠商的聯絡人（系統明細子表 `customer_contact` / `supplier_contact`）透過主表的 `lines` 存檔的
//! 整合測試（#108；docs/architecture.md §7.1、§8.5 `custom_records`「明細子表」、§8.7、§10「主表與明細」）。
//!
//! 每個情境對客戶與廠商各跑一次（[`CUSTOMER`]、[`SUPPLIER`]）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
use hammer_shared::{ApiError, ErrorCode, Id, audit, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// 一組主表與聯絡人明細子表。
struct Master {
    /// 主表的資料類別代碼。
    key: &'static str,
    /// 聯絡人的明細子表代碼。
    detail: &'static str,
    /// 聯絡人的資料表。
    table: &'static str,
    /// 聯絡人指向主表的欄位。
    parent_column: &'static str,
    /// 兩位主要聯絡人時的錯誤訊息。
    primary_message: &'static str,
}

const CUSTOMER: Master = Master {
    key: "customer",
    detail: "customer_contact",
    table: "customer_contacts",
    parent_column: "customer_id",
    primary_message: "每個客戶最多只能有一位主要聯絡人",
};

const SUPPLIER: Master = Master {
    key: "supplier",
    detail: "supplier_contact",
    table: "supplier_contacts",
    parent_column: "supplier_id",
    primary_message: "每個廠商最多只能有一位主要聯絡人",
};

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

async fn create(pool: &PgPool, token: &str, m: &Master, body: Value) -> (StatusCode, Value) {
    call(pool, Method::POST, &records_uri(m.key), token, Some(body)).await
}

async fn update(
    pool: &PgPool,
    token: &str,
    m: &Master,
    id: Id,
    body: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &record_uri(m.key, id),
        token,
        Some(body),
    )
    .await
}

async fn get(pool: &PgPool, token: &str, m: &Master, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(m.key, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

fn record(status: StatusCode, expected: StatusCode, body: Value) -> Record {
    assert_eq!(status, expected, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 回應中聯絡人的 `(id, 姓名)`，依順序。
fn contacts(record: &Record, m: &Master) -> Vec<(Id, String)> {
    record.lines.as_ref().unwrap()[m.detail]
        .iter()
        .map(|l| (l.id, l.data["name"].as_str().unwrap().to_owned()))
        .collect()
}

fn names(record: &Record, m: &Master) -> Vec<String> {
    contacts(record, m).into_iter().map(|(_, n)| n).collect()
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct ContactRow {
    id: Id,
    parent: Id,
    sort_order: i32,
    name: String,
    is_primary: bool,
    custom_data: Value,
    deleted: bool,
}

/// 聯絡人資料表中屬於 `parent` 的所有資料（含已刪除），依 `sort_order`、`id`。
async fn contact_rows(pool: &PgPool, m: &Master, parent: Id) -> Vec<ContactRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, {col} AS parent, sort_order, name, is_primary, custom_data,
                deleted_at IS NOT NULL AS deleted
         FROM {table} WHERE {col} = $1 ORDER BY deleted_at NULLS FIRST, sort_order, id",
        col = m.parent_column,
        table = m.table,
    )))
    .bind(parent)
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

fn lines_key(m: &Master) -> String {
    format!("lines.{}", m.detail)
}

// ---------------------------------------------------------------------------
// 新增、修改、刪除、排序
// ---------------------------------------------------------------------------

/// 新增主表時一起建立聯絡人；修改時整組取代（更新、新增、刪除、排序）；聯絡人有變動時主表的
/// `version` 遞增，操作紀錄記在主表的 diff；沒有變動時不寫入。
async fn contacts_are_saved_with_master(pool: &PgPool, m: &Master) {
    let editor = token_for(pool, "editor", role::EDITOR).await;
    let (status, body) = create(
        pool,
        &editor,
        m,
        json!({
            "data": { "code": "M001", "name": "甲公司" },
            "lines": { (m.detail): [
                { "data": { "name": "王小明", "title": "經理", "is_primary": true } },
                { "data": { "name": "李小華", "mobile": "0912345678" } },
            ] },
        }),
    )
    .await;
    let created = record(status, StatusCode::CREATED, body);
    assert_eq!(created.version, 1);
    assert_eq!(names(&created, m), ["王小明", "李小華"]);
    let lines = &created.lines.as_ref().unwrap()[m.detail];
    assert_eq!(lines[0].title, "王小明");
    assert_eq!(lines[0].data["is_primary"], true);
    // is_primary 預設 false
    assert_eq!(lines[1].data["is_primary"], false);
    assert_eq!(lines[1].data["mobile"], "0912345678");
    assert!(lines[0].record_no.is_none());

    // 資料寫在聯絡人資料表，父資料與順序存在實體欄位
    let rows = contact_rows(pool, m, created.id).await;
    let (wang, lee) = (rows[0].id, rows[1].id);
    assert_eq!(
        rows.iter()
            .map(|r| (
                r.parent,
                r.sort_order,
                r.name.as_str(),
                r.is_primary,
                r.deleted
            ))
            .collect::<Vec<_>>(),
        [
            (created.id, 0, "王小明", true, false),
            (created.id, 1, "李小華", false, false),
        ]
    );
    assert_eq!(rows[0].custom_data, json!({}));
    assert_eq!(
        contacts(&get(pool, &editor, m, created.id).await, m),
        [(wang, "王小明".to_owned()), (lee, "李小華".to_owned()),]
    );

    // 操作紀錄：新增時 after 有聯絡人
    let logs = audit_logs(pool, created.id).await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].action, audit::CREATE);
    assert_eq!(logs[0].entity_key.as_deref(), Some(m.key));
    let after = logs[0].after_data.as_ref().unwrap();
    assert_eq!(after[lines_key(m)][0]["id"], json!(wang));
    assert_eq!(after[lines_key(m)][0]["data"]["name"], "王小明");
    assert_eq!(after[lines_key(m)][1]["id"], json!(lee));

    // 修改：李小華移到第一列並修改職稱、刪除王小明、新增張大同
    let (status, body) = update(
        pool,
        &editor,
        m,
        created.id,
        json!({
            "version": 1,
            "data": {},
            "lines": { (m.detail): [
                { "id": lee, "data": { "title": "業務" } },
                { "data": { "name": "張大同", "is_primary": true } },
            ] },
        }),
    )
    .await;
    let updated = record(status, StatusCode::OK, body);
    assert_eq!(updated.version, 2, "聯絡人變動時主表的 version 遞增");
    assert_eq!(names(&updated, m), ["李小華", "張大同"]);
    let lines = &updated.lines.as_ref().unwrap()[m.detail];
    assert_eq!(lines[0].id, lee);
    assert_eq!(lines[0].data["title"], "業務");
    // 只合併有帶到的欄位
    assert_eq!(lines[0].data["mobile"], "0912345678");
    let chang = lines[1].id;

    let rows = contact_rows(pool, m, created.id).await;
    assert_eq!(
        rows.iter()
            .map(|r| (r.id, r.sort_order, r.deleted))
            .collect::<Vec<_>>(),
        [(lee, 0, false), (chang, 1, false), (wang, 0, true)]
    );

    // 操作紀錄：修改的 diff 有修改前後的聯絡人
    let logs = audit_logs(pool, created.id).await;
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[1].action, audit::UPDATE);
    let before = logs[1].before_data.as_ref().unwrap();
    let after = logs[1].after_data.as_ref().unwrap();
    assert_eq!(
        before.as_object().unwrap().keys().collect::<Vec<_>>(),
        [&lines_key(m)],
        "只有聯絡人變動：{before}"
    );
    assert_eq!(before[lines_key(m)][0]["id"], json!(wang));
    assert_eq!(after[lines_key(m)][0]["id"], json!(lee));
    assert_eq!(after[lines_key(m)][0]["data"]["title"], "業務");
    assert_eq!(after[lines_key(m)][1]["id"], json!(chang));

    // 只調整順序也是變動
    let (status, body) = update(
        pool,
        &editor,
        m,
        created.id,
        json!({
            "version": 2,
            "data": {},
            "lines": { (m.detail): [ { "id": chang, "data": {} }, { "id": lee, "data": {} } ] },
        }),
    )
    .await;
    let reordered = record(status, StatusCode::OK, body);
    assert_eq!(reordered.version, 3);
    assert_eq!(names(&reordered, m), ["張大同", "李小華"]);

    // 沒有變動：不寫入、version 不變
    let (status, body) = update(
        pool,
        &editor,
        m,
        created.id,
        json!({
            "version": 3,
            "data": {},
            "lines": { (m.detail): [ { "id": chang, "data": {} }, { "id": lee, "data": {} } ] },
        }),
    )
    .await;
    assert_eq!(record(status, StatusCode::OK, body).version, 3);
    // 沒帶到明細子表時聯絡人不變
    let (status, body) = update(
        pool,
        &editor,
        m,
        created.id,
        json!({ "version": 3, "data": { "short_name": "甲" } }),
    )
    .await;
    let renamed = record(status, StatusCode::OK, body);
    assert_eq!(renamed.version, 4);
    assert_eq!(names(&renamed, m), ["張大同", "李小華"]);
    assert_eq!(audit_logs(pool, created.id).await.len(), 4);

    // 版本不符
    let (status, body) = update(
        pool,
        &editor,
        m,
        created.id,
        json!({ "version": 3, "data": {}, "lines": { (m.detail): [] } }),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);

    // 清空聯絡人
    let (status, body) = update(
        pool,
        &editor,
        m,
        created.id,
        json!({ "version": 4, "data": {}, "lines": { (m.detail): [] } }),
    )
    .await;
    let cleared = record(status, StatusCode::OK, body);
    assert_eq!(cleared.version, 5);
    assert_eq!(cleared.lines.as_ref().unwrap()[m.detail], vec![]);
    assert!(
        contact_rows(pool, m, created.id)
            .await
            .iter()
            .all(|r| r.deleted)
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_contacts_are_saved_with_customer(pool: PgPool) {
    contacts_are_saved_with_master(&pool, &CUSTOMER).await;
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn supplier_contacts_are_saved_with_supplier(pool: PgPool) {
    contacts_are_saved_with_master(&pool, &SUPPLIER).await;
}

// ---------------------------------------------------------------------------
// 驗證
// ---------------------------------------------------------------------------

/// `details.lines` 的 `(index, field_key, message)`。
fn line_errors(err: &ApiError) -> Vec<(Option<u64>, Option<String>, String)> {
    err.details.as_ref().unwrap()["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["index"].as_u64(),
                e["field_key"].as_str().map(str::to_owned),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// 兩位主要聯絡人時回傳 422，`details.lines` 指出第二列（§7.2）；姓名必填；驗證失敗時不寫入。
async fn at_most_one_primary_contact(pool: &PgPool, m: &Master) {
    let editor = token_for(pool, "editor", role::EDITOR).await;
    let primary = |index| {
        (
            Some(index),
            Some("is_primary".to_owned()),
            m.primary_message.to_owned(),
        )
    };

    // 新增
    let (status, body) = create(
        pool,
        &editor,
        m,
        json!({
            "data": { "code": "M001", "name": "甲公司" },
            "lines": { (m.detail): [
                { "data": { "name": "A", "is_primary": true } },
                { "data": { "name": "B" } },
                { "data": { "name": "C", "is_primary": true } },
            ] },
        }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(line_errors(&err), [primary(2)], "{body}");
    assert_eq!(
        err.details.as_ref().unwrap()["lines"][0]["detail_key"],
        m.detail
    );
    let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM {}",
        m.table
    )))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "驗證失敗時不寫入");

    // 姓名必填；其他列的錯誤一起回傳
    let (status, body) = create(
        pool,
        &editor,
        m,
        json!({
            "data": { "code": "M001", "name": "甲公司" },
            "lines": { (m.detail): [ { "data": { "title": "經理" } } ] },
        }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(line_errors(&err)[0].0, Some(0));
    assert_eq!(line_errors(&err)[0].1.as_deref(), Some("name"));

    // 修改：把既有的第二位改成主要聯絡人
    let (status, body) = create(
        pool,
        &editor,
        m,
        json!({
            "data": { "code": "M001", "name": "甲公司" },
            "lines": { (m.detail): [
                { "data": { "name": "A", "is_primary": true } },
                { "data": { "name": "B" } },
            ] },
        }),
    )
    .await;
    let created = record(status, StatusCode::CREATED, body);
    let ids: Vec<Id> = contacts(&created, m)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let (status, body) = update(
        pool,
        &editor,
        m,
        created.id,
        json!({
            "version": 1,
            "data": {},
            "lines": { (m.detail): [
                { "id": ids[0], "data": {} },
                { "id": ids[1], "data": { "is_primary": true } },
            ] },
        }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(line_errors(&err), [primary(1)], "{body}");
    assert_eq!(get(pool, &editor, m, created.id).await.version, 1);

    // 換主要聯絡人（同一次存檔內取消舊的、設定新的）可以
    let (status, body) = update(
        pool,
        &editor,
        m,
        created.id,
        json!({
            "version": 1,
            "data": {},
            "lines": { (m.detail): [
                { "id": ids[0], "data": { "is_primary": false } },
                { "id": ids[1], "data": { "is_primary": true } },
            ] },
        }),
    )
    .await;
    let updated = record(status, StatusCode::OK, body);
    let lines = &updated.lines.as_ref().unwrap()[m.detail];
    assert_eq!(lines[0].data["is_primary"], false);
    assert_eq!(lines[1].data["is_primary"], true);

    // 不同主表各自可以有一位主要聯絡人
    let (status, body) = create(
        pool,
        &editor,
        m,
        json!({
            "data": { "code": "M002", "name": "乙公司" },
            "lines": { (m.detail): [ { "data": { "name": "C", "is_primary": true } } ] },
        }),
    )
    .await;
    record(status, StatusCode::CREATED, body);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_has_at_most_one_primary_contact(pool: PgPool) {
    at_most_one_primary_contact(&pool, &CUSTOMER).await;
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn supplier_has_at_most_one_primary_contact(pool: PgPool) {
    at_most_one_primary_contact(&pool, &SUPPLIER).await;
}

// ---------------------------------------------------------------------------
// 自訂欄位
// ---------------------------------------------------------------------------

/// 聯絡人的自訂欄位存在聯絡人資料表的 `custom_data`；不可重複的自訂欄位跨主表檢查。
async fn contact_custom_fields(pool: &PgPool, m: &Master) {
    let admin = token_for(pool, "admin", role::ADMIN).await;
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{}/fields", m.detail),
        &admin,
        Some(json!({ "field_key": "line_id", "label": "LINE ID", "data_type": "text", "is_unique": true })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = create(
        pool,
        &admin,
        m,
        json!({
            "data": { "code": "M001", "name": "甲公司" },
            "lines": { (m.detail): [ { "data": { "name": "王小明", "line_id": "wang" } } ] },
        }),
    )
    .await;
    let created = record(status, StatusCode::CREATED, body);
    assert_eq!(
        created.lines.as_ref().unwrap()[m.detail][0].data["line_id"],
        "wang"
    );
    let rows = contact_rows(pool, m, created.id).await;
    assert_eq!(rows[0].custom_data, json!({ "line_id": "wang" }));

    // 修改自訂欄位
    let id = rows[0].id;
    let (status, body) = update(
        pool,
        &admin,
        m,
        created.id,
        json!({
            "version": 1,
            "data": {},
            "lines": { (m.detail): [ { "id": id, "data": { "line_id": "wang2" } } ] },
        }),
    )
    .await;
    record(status, StatusCode::OK, body);
    let rows = contact_rows(pool, m, created.id).await;
    assert_eq!(rows[0].custom_data, json!({ "line_id": "wang2" }));
    assert_eq!(rows[0].name, "王小明");

    // 與其他主表的聯絡人重複 → 422，details.lines 指出列號
    let (status, body) = create(
        pool,
        &admin,
        m,
        json!({
            "data": { "code": "M002", "name": "乙公司" },
            "lines": { (m.detail): [
                { "data": { "name": "李小華", "line_id": "lee" } },
                { "data": { "name": "王大明", "line_id": "wang2" } },
            ] },
        }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(
        line_errors(&err),
        [(
            Some(1),
            Some("line_id".to_owned()),
            "此值已被其他資料使用".to_owned()
        )],
        "{body}"
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_contact_custom_fields(pool: PgPool) {
    contact_custom_fields(&pool, &CUSTOMER).await;
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn supplier_contact_custom_fields(pool: PgPool) {
    contact_custom_fields(&pool, &SUPPLIER).await;
}

// ---------------------------------------------------------------------------
// 刪除
// ---------------------------------------------------------------------------

/// 刪除主表時聯絡人一起軟刪除；操作紀錄的 before 有聯絡人。其他主表的聯絡人不受影響。
async fn contacts_are_deleted_with_master(pool: &PgPool, m: &Master) {
    let editor = token_for(pool, "editor", role::EDITOR).await;
    let mut ids = Vec::new();
    for code in ["M001", "M002"] {
        let (status, body) = create(
            pool,
            &editor,
            m,
            json!({
                "data": { "code": code, "name": code },
                "lines": { (m.detail): [
                    { "data": { "name": "A", "is_primary": true } },
                    { "data": { "name": "B" } },
                ] },
            }),
        )
        .await;
        ids.push(record(status, StatusCode::CREATED, body).id);
    }

    let (status, body) = call(
        pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri(m.key, ids[0])),
        &editor,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let rows = contact_rows(pool, m, ids[0]).await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.deleted), "{rows:?}");
    assert!(
        contact_rows(pool, m, ids[1])
            .await
            .iter()
            .all(|r| !r.deleted)
    );

    let logs = audit_logs(pool, ids[0]).await;
    let delete = logs.last().unwrap();
    assert_eq!(delete.action, audit::DELETE);
    assert!(delete.after_data.is_none());
    let before = delete.before_data.as_ref().unwrap();
    assert_eq!(before[lines_key(m)][0]["id"], json!(rows[0].id));
    assert_eq!(before[lines_key(m)][1]["data"]["name"], "B");

    let (status, body) = call(pool, Method::GET, &record_uri(m.key, ids[0]), &editor, None).await;
    assert_error(status, &body, ErrorCode::NotFound);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_contacts_are_deleted_with_customer(pool: PgPool) {
    contacts_are_deleted_with_master(&pool, &CUSTOMER).await;
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn supplier_contacts_are_deleted_with_supplier(pool: PgPool) {
    contacts_are_deleted_with_master(&pool, &SUPPLIER).await;
}

/// 明細的 `id` 必須是這筆主表的聯絡人；不是聯絡人的明細子表代碼 → 422。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn contact_ids_must_belong_to_master(pool: PgPool) {
    let m = &CUSTOMER;
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let mut created = Vec::new();
    for code in ["M001", "M002"] {
        let (status, body) = create(
            &pool,
            &editor,
            m,
            json!({
                "data": { "code": code, "name": code },
                "lines": { (m.detail): [ { "data": { "name": "A" } } ] },
            }),
        )
        .await;
        created.push(record(status, StatusCode::CREATED, body));
    }
    let other = contacts(&created[1], m)[0].0;
    let (status, body) = update(
        &pool,
        &editor,
        m,
        created[0].id,
        json!({ "version": 1, "data": {}, "lines": { (m.detail): [ { "id": other, "data": {} } ] } }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(line_errors(&err)[0].0, Some(0), "{body}");
    assert_eq!(line_errors(&err)[0].1, None, "{body}");

    // 廠商的聯絡人不是客戶的明細子表
    let (status, body) = update(
        &pool,
        &editor,
        m,
        created[0].id,
        json!({ "version": 1, "data": {}, "lines": { "supplier_contact": [] } }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(line_errors(&err)[0].0, None, "{body}");
}
