//! 欄位資料型態變更 API 的整合測試（#49）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::pagination::Page;
use hammer_shared::record::Record;
use hammer_shared::schema::{DataType, EntityDetail, FieldDefinition};
use hammer_shared::{ApiError, ErrorCode, Id, audit, new_id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const ENTITY: &str = "repair_order";

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

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

/// 建立資料類別 [`ENTITY`]，回傳管理者 token 與資料類別 id。
async fn setup(pool: &PgPool, fields: &[Value]) -> (String, Id) {
    let admin = token_for(pool, "admin", role::ADMIN).await;
    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        &admin,
        Some(json!({ "key": ENTITY, "label": "維修單", "kind": "master" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    for field in fields {
        let (status, body) = call(
            pool,
            Method::POST,
            &format!("/api/v1/entities/{ENTITY}/fields"),
            &admin,
            Some(field.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    (admin, detail.entity.id)
}

async fn get_field(pool: &PgPool, token: &str, field_key: &str) -> FieldDefinition {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{ENTITY}"),
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

async fn create(pool: &PgPool, token: &str, data: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{ENTITY}/records"),
        token,
        Some(json!({ "data": data })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn change_type(
    pool: &PgPool,
    token: &str,
    field_key: &str,
    body: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{ENTITY}/fields/{field_key}/change-type"),
        token,
        Some(body),
    )
    .await
}

/// 依 `sort` 排序列出資料，回傳每筆的 `field_key` 值。
async fn sorted_values(pool: &PgPool, token: &str, sort: &str, field_key: &str) -> Vec<Value> {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/entities/{ENTITY}/records?sort={sort}"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    page.items
        .into_iter()
        .map(|r| r.data.get(field_key).cloned().unwrap_or(Value::Null))
        .collect()
}

/// 直接讀取資料庫中所有資料（含已軟刪除），依 id 排序。
async fn stored(pool: &PgPool, entity_id: Id) -> Vec<(Id, Value, i32)> {
    sqlx::query_as("SELECT id, data, version FROM custom_records WHERE entity_id = $1 ORDER BY id")
        .bind(entity_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn soft_delete(pool: &PgPool, token: &str, record: &Record) {
    let (status, body) = call(
        pool,
        Method::DELETE,
        &format!(
            "/api/v1/entities/{ENTITY}/records/{}?version={}",
            record.id, record.version
        ),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
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

fn text_field(key: &str) -> Value {
    json!({ "field_key": key, "label": key, "data_type": "text" })
}

// ---------------------------------------------------------------------------
// 驗收條件
// ---------------------------------------------------------------------------

/// 有一筆資料無法轉換時整個操作被拒絕，資料與欄位定義都沒有被修改。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn one_failure_rejects_whole_change(pool: PgPool) {
    let (admin, entity_id) = setup(&pool, &[text_field("code")]).await;
    let ok = create(&pool, &admin, json!({ "code": "12" })).await;
    let bad = create(&pool, &admin, json!({ "code": "abc" })).await;
    let field = get_field(&pool, &admin, "code").await;
    let before = stored(&pool, entity_id).await;
    let logs_before = change_logs(&pool, field.id).await.len();

    let (status, body) = change_type(
        &pool,
        &admin,
        "code",
        json!({ "version": field.version, "data_type": "integer" }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    let details = err.details.unwrap();
    assert_eq!(details["field"], json!("data_type"));
    assert_eq!(details["total"], json!(1));
    let records = details["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["id"], json!(bad.id));
    assert_eq!(records[0]["value"], json!("abc"));
    assert_eq!(records[0]["deleted"], json!(false));
    assert!(!records[0]["message"].as_str().unwrap().is_empty());

    // 資料、欄位定義、紀錄都沒有變動
    assert_eq!(stored(&pool, entity_id).await, before);
    let after = get_field(&pool, &admin, "code").await;
    assert_eq!(after.data_type, DataType::Text);
    assert_eq!(after.version, field.version);
    assert_eq!(change_logs(&pool, field.id).await.len(), logs_before);
    assert_eq!(audit_logs(&pool, ok.id).await.len(), 1);
}

/// 轉換成功後，列表依新型態排序；已軟刪除的資料也被轉換；留下變更紀錄。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn converted_records_sort_by_new_type(pool: PgPool) {
    let (admin, entity_id) = setup(&pool, &[text_field("code"), text_field("title")]).await;
    for code in ["10", "9", "100"] {
        create(&pool, &admin, json!({ "code": code, "title": code })).await;
    }
    create(&pool, &admin, json!({ "title": "沒有代碼" })).await;
    let deleted = create(&pool, &admin, json!({ "code": " 5 " })).await;
    soft_delete(&pool, &admin, &deleted).await;

    // 文字排序
    assert_eq!(
        sorted_values(&pool, &admin, "code", "code").await,
        [json!("10"), json!("100"), json!("9"), Value::Null]
    );

    let field = get_field(&pool, &admin, "code").await;
    let (status, body) = change_type(
        &pool,
        &admin,
        "code",
        json!({ "version": field.version, "data_type": "integer",
                "validation": { "min": "0" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let changed: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(changed.data_type, DataType::Integer);
    assert_eq!(changed.version, field.version + 1);
    assert_eq!(changed.validation.unwrap().min.unwrap().to_string(), "0");

    // 依數值排序
    assert_eq!(
        sorted_values(&pool, &admin, "code", "code").await,
        [json!(9), json!(10), json!(100), Value::Null]
    );
    assert_eq!(
        sorted_values(&pool, &admin, "-code", "code").await,
        [json!(100), json!(10), json!(9), Value::Null]
    );

    // 已軟刪除的資料也已轉換；有變動的資料 version 遞增，沒有值的資料不變
    let rows = stored(&pool, entity_id).await;
    let deleted_row = rows.iter().find(|(id, ..)| *id == deleted.id).unwrap();
    assert_eq!(deleted_row.1["code"], json!(5));
    for (_, data, version) in &rows {
        match data.get("code") {
            Some(code) => {
                assert!(code.is_i64(), "{data}");
                assert!(*version >= 2, "{data}");
            }
            None => assert_eq!(*version, 1, "{data}"),
        }
    }

    // 新增資料使用新型態驗證
    create(&pool, &admin, json!({ "code": 7 })).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/entities/{ENTITY}/records"),
        &admin,
        Some(json!({ "data": { "code": "7" } })),
    )
    .await;
    assert_error(status, &body, ErrorCode::Validation);

    // schema_change_logs 與 audit_logs
    let logs = change_logs(&pool, field.id).await;
    let last = logs.last().unwrap();
    assert_eq!(last.action, "update");
    assert_eq!(
        last.before_data.as_ref().unwrap()["data_type"],
        json!("text")
    );
    assert_eq!(
        last.after_data.as_ref().unwrap()["data_type"],
        json!("integer")
    );
    let audits = audit_logs(&pool, field.id).await;
    let last = audits.last().unwrap();
    assert_eq!(last.action, audit::UPDATE);
    assert_eq!(last.entity_key.as_deref(), Some("field_definition"));
    assert_eq!(
        last.before_data.as_ref().unwrap()["data_type"],
        json!("text")
    );
    assert_eq!(
        last.after_data.as_ref().unwrap()["data_type"],
        json!("integer")
    );
}

/// 系統欄位變更型態被拒絕。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn system_field_cannot_change_type(pool: PgPool) {
    let (admin, entity_id) = setup(&pool, &[]).await;
    sqlx::query(
        "INSERT INTO field_definitions
             (id, entity_id, field_key, label, data_type, is_system, storage, column_name)
         VALUES ($1, $2, 'name', '名稱', 'text', true, 'column', 'name')",
    )
    .bind(new_id())
    .bind(entity_id)
    .execute(&pool)
    .await
    .unwrap();
    let field = get_field(&pool, &admin, "name").await;

    let (status, body) = change_type(
        &pool,
        &admin,
        "name",
        json!({ "version": field.version, "data_type": "long_text" }),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": "data_type" })));
    let after = get_field(&pool, &admin, "name").await;
    assert_eq!(after.data_type, DataType::Text);
    assert_eq!(after.version, field.version);
    assert!(change_logs(&pool, field.id).await.is_empty());
}

// ---------------------------------------------------------------------------
// 其他
// ---------------------------------------------------------------------------

/// 失敗的資料最多列 100 筆，並附上失敗總筆數；已軟刪除的資料也會擋下轉換。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn failures_are_capped_with_total(pool: PgPool) {
    let (admin, entity_id) = setup(&pool, &[text_field("code")]).await;
    for i in 0..105 {
        sqlx::query(
            "INSERT INTO custom_records (id, entity_id, data, deleted_at)
             VALUES ($1, $2, $3, CASE WHEN $4 THEN now() END)",
        )
        .bind(new_id())
        .bind(entity_id)
        .bind(json!({ "code": format!("x{i}") }))
        .bind(i == 0)
        .execute(&pool)
        .await
        .unwrap();
    }
    let field = get_field(&pool, &admin, "code").await;
    let (status, body) = change_type(
        &pool,
        &admin,
        "code",
        json!({ "version": field.version, "data_type": "decimal", "decimal_places": 2 }),
    )
    .await;
    let details = assert_error(status, &body, ErrorCode::Validation)
        .details
        .unwrap();
    assert_eq!(details["total"], json!(105));
    let records = details["records"].as_array().unwrap();
    assert_eq!(records.len(), 100);
    assert_eq!(records[0]["value"], json!("x0"));
    assert_eq!(records[0]["deleted"], json!(true));
}

/// 安全的轉換（select → multi_select）會轉換預設值；小數位數不夠時拒絕，不自動四捨五入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn safe_change_converts_default_and_strict_change_checks_places(pool: PgPool) {
    let options = json!([
        { "value": "open", "label": "處理中" },
        { "value": "closed", "label": "已結案" },
    ]);
    let (admin, _) = setup(
        &pool,
        &[
            json!({ "field_key": "status", "label": "狀態", "data_type": "select",
                    "options": options, "default_value": "open" }),
            json!({ "field_key": "amount", "label": "金額", "data_type": "decimal",
                    "decimal_places": 2, "is_unique": true }),
        ],
    )
    .await;
    let record = create(&pool, &admin, json!({ "amount": "1.00" })).await;
    create(
        &pool,
        &admin,
        json!({ "status": "closed", "amount": "1.50" }),
    )
    .await;

    let field = get_field(&pool, &admin, "status").await;
    let (status, body) = change_type(
        &pool,
        &admin,
        "status",
        json!({ "version": field.version, "data_type": "multi_select", "options": options }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let changed: FieldDefinition = serde_json::from_value(body).unwrap();
    assert_eq!(changed.default_value, Some(json!(["open"])));
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{ENTITY}/records/{}", record.id),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], json!(["open"]));

    // 1.00 與 1.50 轉成整數：1.50 有小數無法轉換
    let field = get_field(&pool, &admin, "amount").await;
    let (status, body) = change_type(
        &pool,
        &admin,
        "amount",
        json!({ "version": field.version, "data_type": "integer" }),
    )
    .await;
    let details = assert_error(status, &body, ErrorCode::Validation)
        .details
        .unwrap();
    assert_eq!(details["total"], json!(1));

    // 轉成 0 位小數的金額：1.50 無法轉換（不自動四捨五入）
    let (status, body) = change_type(
        &pool,
        &admin,
        "amount",
        json!({ "version": field.version, "data_type": "money", "decimal_places": 0 }),
    )
    .await;
    let details = assert_error(status, &body, ErrorCode::Validation)
        .details
        .unwrap();
    assert_eq!(details["total"], json!(1), "{details}");
    assert_eq!(details["records"][0]["value"], json!("1.50"));
}

/// 不可重複的欄位，轉換後重複的資料都列為失敗；已軟刪除的資料不算重複。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unique_field_rejects_duplicates_after_conversion(pool: PgPool) {
    let (admin, _) = setup(
        &pool,
        &[json!({ "field_key": "ref", "label": "編號", "data_type": "text", "is_unique": true })],
    )
    .await;
    let a = create(&pool, &admin, json!({ "ref": "1" })).await;
    let b = create(&pool, &admin, json!({ "ref": "01" })).await;
    create(&pool, &admin, json!({ "ref": "2" })).await;
    let c = create(&pool, &admin, json!({ "ref": "02" })).await;
    soft_delete(&pool, &admin, &c).await;

    let field = get_field(&pool, &admin, "ref").await;
    let (status, body) = change_type(
        &pool,
        &admin,
        "ref",
        json!({ "version": field.version, "data_type": "integer" }),
    )
    .await;
    let details = assert_error(status, &body, ErrorCode::Validation)
        .details
        .unwrap();
    assert_eq!(details["total"], json!(2), "{details}");
    let ids: Vec<_> = details["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].clone())
        .collect();
    assert_eq!(ids, [json!(a.id), json!(b.id)]);
}

/// 需要 `schema.manage`；版本不符回傳 409；設定不合法回傳 422。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn permission_version_and_settings(pool: PgPool) {
    let (admin, _) = setup(&pool, &[text_field("code")]).await;
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let field = get_field(&pool, &admin, "code").await;

    let (status, body) = change_type(
        &pool,
        &editor,
        "code",
        json!({ "version": field.version, "data_type": "long_text" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::Forbidden);

    let (status, body) = change_type(
        &pool,
        &admin,
        "code",
        json!({ "version": field.version + 1, "data_type": "long_text" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);

    for (req, param) in [
        (json!({ "data_type": "decimal" }), "decimal_places"),
        (json!({ "data_type": "text" }), "data_type"),
        (json!({ "data_type": "relation" }), "data_type"),
    ] {
        let mut req = req;
        req["version"] = json!(field.version);
        let (status, body) = change_type(&pool, &admin, "code", req).await;
        let err = assert_error(status, &body, ErrorCode::Validation);
        assert_eq!(err.details, Some(json!({ "field": param })), "{body}");
    }

    let (status, body) = change_type(
        &pool,
        &admin,
        "missing",
        json!({ "version": 1, "data_type": "long_text" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);
}
