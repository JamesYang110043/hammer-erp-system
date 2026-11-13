//! 自訂資料類別的通用資料 API 的整合測試（#48）。
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
use hammer_shared::{ApiError, ErrorCode, Id, audit, role};
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

/// 422 VALIDATION，且 `details.field` 為 `param`。
fn assert_invalid_param(status: StatusCode, body: &Value, param: &str) {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": param })), "{body}");
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

/// 建立資料類別 [`ENTITY`] 與以下欄位，回傳管理者 token。
///
/// | 欄位 | 型態 | 設定 |
/// |---|---|---|
/// | `title` | text | 必填 |
/// | `note` | long_text | |
/// | `qty` | integer | 0 到 1000 |
/// | `amount` | decimal | 2 位小數 |
/// | `due` | date | |
/// | `done` | boolean | |
/// | `status` | select | open / closed |
/// | `tags` | multi_select | a / b / c |
async fn setup(pool: &PgPool) -> String {
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

    let options = |values: &[&str]| -> Value {
        values
            .iter()
            .map(|v| json!({ "value": v, "label": v.to_uppercase() }))
            .collect()
    };
    for field in [
        json!({ "field_key": "title", "label": "標題", "data_type": "text", "is_required": true }),
        json!({ "field_key": "note", "label": "備註", "data_type": "long_text" }),
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer",
                "validation": { "min": "0", "max": "1000" } }),
        json!({ "field_key": "amount", "label": "金額", "data_type": "decimal", "decimal_places": 2 }),
        json!({ "field_key": "due", "label": "到期日", "data_type": "date" }),
        json!({ "field_key": "done", "label": "完成", "data_type": "boolean" }),
        json!({ "field_key": "status", "label": "狀態", "data_type": "select",
                "options": options(&["open", "closed"]) }),
        json!({ "field_key": "tags", "label": "標籤", "data_type": "multi_select",
                "options": options(&["a", "b", "c"]) }),
    ] {
        let (status, body) = call(
            pool,
            Method::POST,
            &format!("/api/v1/entities/{ENTITY}/fields"),
            &admin,
            Some(field),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    admin
}

fn records_uri() -> String {
    format!("/api/v1/entities/{ENTITY}/records")
}

fn record_uri(id: Id) -> String {
    format!("/api/v1/entities/{ENTITY}/records/{id}")
}

async fn create(pool: &PgPool, token: &str, data: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &records_uri(),
        token,
        Some(json!({ "data": data })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
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

async fn list_raw(pool: &PgPool, token: &str, params: &[(&str, &str)]) -> (StatusCode, Value) {
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    call(
        pool,
        Method::GET,
        &format!("{}?{}", records_uri(), query.join("&")),
        token,
        None,
    )
    .await
}

async fn list(pool: &PgPool, token: &str, params: &[(&str, &str)]) -> Page<Record> {
    let (status, body) = list_raw(pool, token, params).await;
    assert_eq!(status, StatusCode::OK, "{params:?} {body}");
    serde_json::from_value(body).unwrap()
}

/// 列表中每筆資料的 `title`，依回傳順序。
async fn titles(pool: &PgPool, token: &str, params: &[(&str, &str)]) -> Vec<String> {
    list(pool, token, params)
        .await
        .items
        .iter()
        .map(|r| r.data["title"].as_str().unwrap().to_owned())
        .collect()
}

#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    action: String,
    entity_key: Option<String>,
    user_id: Option<Id>,
    before_data: Option<Value>,
    after_data: Option<Value>,
    client_info: Option<String>,
}

async fn audit_logs(pool: &PgPool, record_id: Id) -> Vec<AuditRow> {
    sqlx::query_as(
        "SELECT action, entity_key, user_id, before_data, after_data, client_info
         FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// 驗收條件
// ---------------------------------------------------------------------------

/// 新增、讀取、修改、刪除完整流程，每個寫入都有操作紀錄（含 diff）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn crud_flow_writes_audit_logs(pool: PgPool) {
    let admin = setup(&pool).await;
    let (admin_id,): (Id,) = sqlx::query_as("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();

    // 新增：沒有值的欄位不出現，decimal 補零
    let created = create(
        &pool,
        &admin,
        json!({ "title": "冷氣維修", "qty": 2, "amount": "10.5", "note": "", "tags": ["a"] }),
    )
    .await;
    assert_eq!(
        Value::Object(created.data.clone()),
        json!({ "title": "冷氣維修", "qty": 2, "amount": "10.50", "tags": ["a"] })
    );
    assert_eq!(created.version, 1);
    assert_eq!(created.created_by, Some(admin_id));

    // 讀取
    let (status, body) = call(&pool, Method::GET, &record_uri(created.id), &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(serde_json::from_value::<Record>(body).unwrap(), created);

    // 修改：只合併有帶到的欄位，null 代表清空
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(created.id),
        &admin,
        Some(json!({ "version": 1, "data": { "qty": 3, "tags": null, "status": "open" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: Record = serde_json::from_value(body).unwrap();
    assert_eq!(
        Value::Object(updated.data.clone()),
        json!({ "title": "冷氣維修", "qty": 3, "amount": "10.50", "status": "open" })
    );
    assert_eq!(updated.version, 2);

    // 沒有變動的修改不寫入，version 不變
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(created.id),
        &admin,
        Some(json!({ "version": 2, "data": { "qty": 3 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], 2);

    // 刪除
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=2", record_uri(created.id)),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = call(&pool, Method::GET, &record_uri(created.id), &admin, None).await;
    assert_error(status, &body, ErrorCode::NotFound);
    // 已刪除的資料不能再修改或刪除
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(created.id),
        &admin,
        Some(json!({ "version": 3, "data": { "qty": 4 } })),
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);

    // 操作紀錄：新增、修改、刪除各一筆
    let logs = audit_logs(&pool, created.id).await;
    let actions: Vec<_> = logs.iter().map(|l| l.action.as_str()).collect();
    assert_eq!(actions, [audit::CREATE, audit::UPDATE, audit::DELETE]);
    for log in &logs {
        assert_eq!(log.entity_key.as_deref(), Some(ENTITY));
        assert_eq!(log.user_id, Some(admin_id));
        assert_eq!(log.client_info.as_deref(), Some("HammerERP-test / 0.1.0"));
    }
    assert_eq!(logs[0].before_data, None);
    assert_eq!(
        logs[0].after_data,
        Some(json!({ "title": "冷氣維修", "qty": 2, "amount": "10.50", "tags": ["a"] }))
    );
    // 修改只記錄有變動的欄位
    assert_eq!(
        logs[1].before_data,
        Some(json!({ "qty": 2, "tags": ["a"], "status": null }))
    );
    assert_eq!(
        logs[1].after_data,
        Some(json!({ "qty": 3, "tags": null, "status": "open" }))
    );
    assert_eq!(
        logs[2].before_data,
        Some(json!({ "title": "冷氣維修", "qty": 3, "amount": "10.50", "status": "open" }))
    );
    assert_eq!(logs[2].after_data, None);
}

/// viewer 新增資料得到 403；viewer 可以讀取。editor 可以寫入。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn viewer_can_read_but_not_write(pool: PgPool) {
    let admin = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let record = create(&pool, &admin, json!({ "title": "A" })).await;

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(),
        &viewer,
        Some(json!({ "data": { "title": "B" } })),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Forbidden);
    assert_eq!(err.details, Some(json!({ "permission": "record.write" })));
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(record.id),
        &viewer,
        Some(json!({ "version": 1, "data": { "title": "B" } })),
    )
    .await;
    assert_error(status, &body, ErrorCode::Forbidden);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri(record.id)),
        &viewer,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::Forbidden);

    // viewer 可以讀取
    let page = list(&pool, &viewer, &[]).await;
    assert_eq!(page.total, 1);
    assert_eq!(page.items, std::slice::from_ref(&record));
    let (status, body) = call(&pool, Method::GET, &record_uri(record.id), &viewer, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 被拒絕的寫入沒有改變資料、沒有留紀錄
    assert_eq!(
        list(&pool, &admin, &[]).await.items,
        std::slice::from_ref(&record)
    );
    assert_eq!(audit_logs(&pool, record.id).await.len(), 1);

    // editor 可以寫入
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(record.id),
        &editor,
        Some(json!({ "version": 1, "data": { "title": "B" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 驗證錯誤回傳 422，`details` 列出所有出錯的欄位。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn validation_errors_list_all_fields(pool: PgPool) {
    let admin = setup(&pool).await;

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(),
        &admin,
        Some(json!({ "data": {
            "qty": 1001,
            "amount": "1.234",
            "due": "2026/11/13",
            "status": "gone",
            "unknown": 1,
        } })),
    )
    .await;
    assert_invalid_fields(
        status,
        &body,
        &["title", "qty", "amount", "due", "status", "unknown"],
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM custom_records")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    // 修改時同樣列出所有錯誤，資料不變
    let record = create(&pool, &admin, json!({ "title": "A" })).await;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(record.id),
        &admin,
        Some(json!({ "version": 1, "data": { "title": null, "done": "yes" } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["title", "done"]);
    let (_, body) = call(&pool, Method::GET, &record_uri(record.id), &admin, None).await;
    assert_eq!(body["version"], 1);
    assert_eq!(body["data"], json!({ "title": "A" }));
}

/// `version` 不符時回傳 409。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn stale_version_is_conflict(pool: PgPool) {
    let admin = setup(&pool).await;
    let record = create(&pool, &admin, json!({ "title": "A" })).await;

    let patch =
        |version: i32, title: &str| json!({ "version": version, "data": { "title": title } });
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(record.id),
        &admin,
        Some(patch(1, "B")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 用舊的版本號修改、刪除
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(record.id),
        &admin,
        Some(patch(1, "C")),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri(record.id)),
        &admin,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);

    let (_, body) = call(&pool, Method::GET, &record_uri(record.id), &admin, None).await;
    assert_eq!(body["data"]["title"], "B");
    assert_eq!(body["version"], 2);
}

/// `integer` 欄位排序時 10 排在 9 後面；其他型態也依型態排序，沒有值的排在最後。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn sort_uses_field_type(pool: PgPool) {
    let admin = setup(&pool).await;
    for (title, qty, amount, due) in [
        ("q9", Some(9), "9.00", "2026-02-01"),
        ("q10", Some(10), "10.00", "2026-10-01"),
        ("q2", Some(2), "100.00", "2025-12-31"),
        ("none", None, "-1", "2026-01-15"),
    ] {
        let mut data = json!({ "title": title, "amount": amount, "due": due });
        if let Some(qty) = qty {
            data["qty"] = json!(qty);
        }
        create(&pool, &admin, data).await;
    }

    assert_eq!(
        titles(&pool, &admin, &[("sort", "qty")]).await,
        ["q2", "q9", "q10", "none"]
    );
    assert_eq!(
        titles(&pool, &admin, &[("sort", "-qty")]).await,
        ["q10", "q9", "q2", "none"]
    );
    // decimal 依數值排序（字串排序時 "10.00" < "9.00"）
    assert_eq!(
        titles(&pool, &admin, &[("sort", "amount")]).await,
        ["none", "q9", "q10", "q2"]
    );
    assert_eq!(
        titles(&pool, &admin, &[("sort", "-due")]).await,
        ["q10", "q9", "none", "q2"]
    );
    // 沒有指定排序：依建立順序由新到舊
    assert_eq!(
        titles(&pool, &admin, &[]).await,
        ["none", "q2", "q10", "q9"]
    );

    // 分頁時排序穩定
    let page = list(
        &pool,
        &admin,
        &[("sort", "qty"), ("page", "2"), ("page_size", "3")],
    )
    .await;
    assert_eq!(page.total, 4);
    assert_eq!(page.page, 2);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].data["title"], "none");

    // 不能依未定義的欄位排序
    let (status, body) = list_raw(&pool, &admin, &[("sort", "missing")]).await;
    assert_invalid_param(status, &body, "sort");
}

/// 篩選案例：(query string 參數, 符合的資料 `title`)。
type FilterCase<'a> = (&'a [(&'a str, &'a str)], &'a [&'a str]);

/// 每一種篩選運算子各至少一個案例。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn every_filter_operator(pool: PgPool) {
    let admin = setup(&pool).await;
    create(
        &pool,
        &admin,
        json!({ "title": "冷氣維修", "note": "100% 完成", "qty": 5, "amount": "10.00",
                "due": "2026-11-01", "done": true, "status": "open", "tags": ["a", "b"] }),
    )
    .await;
    create(
        &pool,
        &admin,
        json!({ "title": "冰箱維修", "note": "待料 ABC", "qty": 10, "amount": "20.50",
                "due": "2026-11-30", "done": false, "status": "closed", "tags": ["c"] }),
    )
    .await;
    create(&pool, &admin, json!({ "title": "其他" })).await;

    let cases: &[FilterCase] = &[
        // eq
        (&[("filter.title.eq", "冰箱維修")], &["冰箱維修"]),
        (&[("filter.qty.eq", "5")], &["冷氣維修"]),
        (&[("filter.amount.eq", "20.5")], &["冰箱維修"]),
        (&[("filter.due.eq", "2026-11-30")], &["冰箱維修"]),
        (&[("filter.done.eq", "false")], &["冰箱維修"]),
        (&[("filter.status.eq", "open")], &["冷氣維修"]),
        // contains：不分大小寫；% 是一般字元
        (
            &[("filter.title.contains", "維修")],
            &["冷氣維修", "冰箱維修"],
        ),
        (&[("filter.note.contains", "100%")], &["冷氣維修"]),
        (&[("filter.note.contains", "0%")], &["冷氣維修"]),
        (&[("filter.note.contains", "abc")], &["冰箱維修"]),
        (&[("filter.note.contains", "%料")], &[]),
        // 區間
        (&[("filter.qty.gt", "5")], &["冰箱維修"]),
        (&[("filter.qty.gte", "5")], &["冷氣維修", "冰箱維修"]),
        (&[("filter.qty.lt", "10")], &["冷氣維修"]),
        (&[("filter.qty.lte", "10")], &["冷氣維修", "冰箱維修"]),
        (&[("filter.amount.gte", "10.5")], &["冰箱維修"]),
        (
            &[
                ("filter.due.gte", "2026-11-01"),
                ("filter.due.lt", "2026-11-30"),
            ],
            &["冷氣維修"],
        ),
        // empty
        (&[("filter.qty.empty", "true")], &["其他"]),
        (&[("filter.qty.empty", "false")], &["冷氣維修", "冰箱維修"]),
        // any：同一個欄位的多個值符合任一個即可
        (&[("filter.tags.any", "b")], &["冷氣維修"]),
        (
            &[("filter.tags.any", "a"), ("filter.tags.any", "c")],
            &["冷氣維修", "冰箱維修"],
        ),
        // 多個條件須全部符合
        (
            &[
                ("filter.title.contains", "維修"),
                ("filter.done.eq", "true"),
            ],
            &["冷氣維修"],
        ),
        // 空字串的條件視為未指定
        (
            &[("filter.title.eq", "")],
            &["冷氣維修", "冰箱維修", "其他"],
        ),
    ];
    for (params, expected) in cases {
        let mut params = params.to_vec();
        // 依建立順序，方便比對
        params.push(("sort", "due"));
        let mut actual = titles(&pool, &admin, &params).await;
        let mut expected: Vec<_> = expected.iter().map(|s| (*s).to_owned()).collect();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "{params:?}");
        let page = list(&pool, &admin, &params).await;
        assert_eq!(page.total as usize, expected.len(), "{params:?}");
    }
}

/// 篩選與分頁參數不合法。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_list_params(pool: PgPool) {
    let admin = setup(&pool).await;
    for (param, value) in [
        ("filter.qty.contains", "1"),
        ("filter.qty.gte", "abc"),
        ("filter.tags.eq", "a"),
        ("filter.missing.eq", "1"),
        ("filter.qty.between", "1"),
        ("filter.qty", "1"),
        ("filter.done.empty", "yes"),
    ] {
        let (status, body) = list_raw(&pool, &admin, &[(param, value)]).await;
        assert_invalid_param(status, &body, param);
    }
    let (status, body) = list_raw(&pool, &admin, &[("page", "0")]).await;
    assert_invalid_param(status, &body, "page");
    let (status, body) = list_raw(&pool, &admin, &[("page", "abc")]).await;
    assert_error(status, &body, ErrorCode::BadRequest);
}

/// 已刪除的資料不會出現在列表。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn deleted_records_are_not_listed(pool: PgPool) {
    let admin = setup(&pool).await;
    let a = create(&pool, &admin, json!({ "title": "A", "qty": 1 })).await;
    create(&pool, &admin, json!({ "title": "B", "qty": 1 })).await;

    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri(a.id)),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let page = list(&pool, &admin, &[]).await;
    assert_eq!(page.total, 1);
    assert_eq!(titles(&pool, &admin, &[]).await, ["B"]);
    assert_eq!(
        titles(&pool, &admin, &[("filter.qty.eq", "1")]).await,
        ["B"]
    );
    // 資料庫中仍保留（軟刪除）
    let deleted: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT deleted_at FROM custom_records WHERE id = $1")
            .bind(a.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deleted.is_some());
}

/// 資料類別不存在或已停用時回傳 404；資料屬於其他資料類別時也是 404。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn missing_or_inactive_entity_is_not_found(pool: PgPool) {
    let admin = setup(&pool).await;
    let record = create(&pool, &admin, json!({ "title": "A" })).await;

    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/entities/nothing/records",
        &admin,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);

    // 另一個資料類別讀不到這筆資料
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/entities",
        &admin,
        Some(json!({ "key": "other", "label": "其他", "kind": "master" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/other/records/{}", record.id),
        &admin,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);

    // 停用資料類別
    let (_, detail) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{ENTITY}"),
        &admin,
        None,
    )
    .await;
    let e = &detail["entity"];
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &format!("/api/v1/entities/{ENTITY}"),
        &admin,
        Some(json!({
            "version": e["version"], "label": e["label"], "icon": null,
            "title_template": null, "sort_order": 0, "is_active": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for (method, uri, body) in [
        (Method::GET, records_uri(), None),
        (Method::GET, record_uri(record.id), None),
        (
            Method::POST,
            records_uri(),
            Some(json!({ "data": { "title": "B" } })),
        ),
        (
            Method::PATCH,
            record_uri(record.id),
            Some(json!({ "version": 1, "data": { "title": "B" } })),
        ),
        (
            Method::DELETE,
            format!("{}?version=1", record_uri(record.id)),
            None,
        ),
    ] {
        let (status, res) = call(&pool, method.clone(), &uri, &admin, body).await;
        assert_error(status, &res, ErrorCode::NotFound);
    }
}

/// 違反唯一索引時回傳 422，並指出欄位。
///
/// 唯一索引由 #50 依欄位設定建立；這裡直接建立 §7.5 格式的索引來測試錯誤轉換。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unique_violation_points_to_field(pool: PgPool) {
    let admin = setup(&pool).await;
    sqlx::query("UPDATE field_definitions SET is_unique = true WHERE field_key = 'title'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE UNIQUE INDEX test_uq_title ON custom_records (entity_id, (data->>'title'))
         WHERE deleted_at IS NULL",
    )
    .execute(&pool)
    .await
    .unwrap();

    let a = create(&pool, &admin, json!({ "title": "A" })).await;
    let b = create(&pool, &admin, json!({ "title": "B" })).await;

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(),
        &admin,
        Some(json!({ "data": { "title": "A" } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["title"]);

    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(b.id),
        &admin,
        Some(json!({ "version": 1, "data": { "title": "A" } })),
    )
    .await;
    assert_invalid_fields(status, &body, &["title"]);

    // 失敗的寫入沒有留下操作紀錄
    assert_eq!(audit_logs(&pool, b.id).await.len(), 1);

    // 刪除後可以再使用相同的值
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri(a.id)),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    create(&pool, &admin, json!({ "title": "A" })).await;
}
