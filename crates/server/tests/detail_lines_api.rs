//! 明細子表的 API 整合測試（#72）：建立 `kind = detail` 的資料類別、主表與明細一起新增與修改
//! （整組取代）、驗證錯誤指出列號與欄位、樂觀鎖定、明細不能直接存取、刪除主表時明細一起刪除、
//! 有單號規則的主表取號，以及操作紀錄。
//!
//! 明細的比對與檢查邏輯的單元測試在 `hammer_core::record_lines`。
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::pagination::Page;
use hammer_shared::record::Record;
use hammer_shared::schema::EntityDetail;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const MASTER: &str = "repair_order";

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
fn assert_invalid_field(status: StatusCode, body: &Value, field: &str) {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert_eq!(err.details.unwrap()["field"], field, "{body}");
}

/// 422 VALIDATION，回傳 `details.lines`。
fn invalid_lines(status: StatusCode, body: &Value) -> Vec<Value> {
    let err = assert_error(status, body, ErrorCode::Validation);
    err.details.unwrap()["lines"].as_array().unwrap().clone()
}

async fn admin_token(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    token.into_string()
}

async fn create_entity(pool: &PgPool, token: &str, body: Value) -> EntityDetail {
    let (status, body) = call(pool, Method::POST, "/api/v1/entities", token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_field(pool: &PgPool, token: &str, entity: &str, body: Value) {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{entity}/fields"),
        token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// 建立主表 [`MASTER`]（單號規則 `pattern`，文字欄位 `title`）與兩個明細子表：
///
/// - `parts`（更換零件）：`part`（文字、必填）、`qty`（整數）、`sn`（文字、不可重複）
/// - `labor`（工時）：`task`（文字）、`hours`（小數 1 位）
///
/// 回傳管理者的 token。
async fn setup(pool: &PgPool, pattern: Option<&str>) -> String {
    let token = admin_token(pool).await;
    let master = create_entity(
        pool,
        &token,
        json!({ "key": MASTER, "label": "維修單", "kind": "master", "numbering_pattern": pattern }),
    )
    .await;
    create_field(
        pool,
        &token,
        MASTER,
        json!({ "field_key": "title", "label": "標題", "data_type": "text" }),
    )
    .await;

    for (key, label) in [("parts", "更換零件"), ("labor", "工時")] {
        let detail = create_entity(
            pool,
            &token,
            json!({
                "key": key, "label": label, "kind": "detail",
                "parent_entity_id": master.entity.id,
            }),
        )
        .await;
        assert_eq!(detail.entity.parent_entity_id, Some(master.entity.id));
    }
    create_field(
        pool,
        &token,
        "parts",
        json!({ "field_key": "part", "label": "零件", "data_type": "text", "is_required": true }),
    )
    .await;
    create_field(
        pool,
        &token,
        "parts",
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer" }),
    )
    .await;
    create_field(
        pool,
        &token,
        "parts",
        json!({ "field_key": "sn", "label": "序號", "data_type": "text", "is_unique": true }),
    )
    .await;
    create_field(
        pool,
        &token,
        "labor",
        json!({ "field_key": "task", "label": "項目", "data_type": "text" }),
    )
    .await;
    create_field(
        pool,
        &token,
        "labor",
        json!({ "field_key": "hours", "label": "工時", "data_type": "decimal", "decimal_places": 1 }),
    )
    .await;
    token
}

fn records_uri(entity: &str) -> String {
    format!("/api/v1/entities/{entity}/records")
}

fn record_uri(id: Id) -> String {
    format!("{}/{id}", records_uri(MASTER))
}

async fn create(pool: &PgPool, token: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(MASTER), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn update(pool: &PgPool, token: &str, id: Id, body: Value) -> Record {
    let (status, body) = call(pool, Method::PATCH, &record_uri(id), token, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get(pool: &PgPool, token: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 一個明細子表的明細。
fn lines<'a>(record: &'a Record, key: &str) -> &'a [Record] {
    &record.lines.as_ref().unwrap()[key]
}

/// 明細某個欄位的值，依順序。
fn column(record: &Record, key: &str, field: &str) -> Vec<Value> {
    lines(record, key)
        .iter()
        .map(|l| l.data.get(field).cloned().unwrap_or(Value::Null))
        .collect()
}

/// `custom_records` 的總筆數與未刪除的筆數。
async fn record_counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE deleted_at IS NULL) FROM custom_records",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// 主表資料的操作紀錄 `(action, before_data, after_data)`，依時間排序。
async fn audit_logs(pool: &PgPool, record_id: Id) -> Vec<(String, Option<Value>, Option<Value>)> {
    sqlx::query_as(
        "SELECT action, before_data, after_data FROM audit_logs
         WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn audit_lines(lines: &[Record]) -> Value {
    Value::Array(
        lines
            .iter()
            .map(|l| json!({ "id": l.id, "data": l.data }))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// 驗收條件
// ---------------------------------------------------------------------------

/// 新增主表與兩個子表的明細，讀取後內容一致；列表不回傳明細；操作紀錄包含明細。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn create_with_two_details_and_read_back(pool: PgPool) {
    let token = setup(&pool, None).await;
    let created = create(
        &pool,
        &token,
        json!({
            "data": { "title": "冷氣維修" },
            "lines": {
                "parts": [
                    { "data": { "part": "濾網", "qty": 2 } },
                    { "data": { "part": "壓縮機", "qty": 1, "sn": "C-001" } },
                ],
                "labor": [ { "data": { "task": "拆裝", "hours": "1.5" } } ],
            },
        }),
    )
    .await;
    assert_eq!(created.version, 1);
    assert_eq!(
        column(&created, "parts", "part"),
        [json!("濾網"), json!("壓縮機")]
    );
    assert_eq!(column(&created, "parts", "qty"), [json!(2), json!(1)]);
    assert_eq!(column(&created, "labor", "hours"), [json!("1.5")]);
    for line in lines(&created, "parts") {
        assert_eq!(line.record_no, None);
        assert_eq!(line.lines, None);
        assert_eq!(line.version, 1);
    }
    // 明細的標題依明細子表的預設規則（第一個文字欄位）
    assert_eq!(lines(&created, "parts")[0].title, "濾網");

    let fetched = get(&pool, &token, created.id).await;
    assert_eq!(fetched, created);

    // 列表不回傳明細
    let (status, body) = call(&pool, Method::GET, &records_uri(MASTER), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"][0].get("lines").is_none(), "{body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    assert_eq!(page.total, 1);

    // 沒有明細的主表：每個有效的明細子表都是空陣列
    let empty = create(&pool, &token, json!({ "data": { "title": "空" } })).await;
    assert_eq!(
        get(&pool, &token, empty.id).await.lines,
        Some([("labor".to_owned(), vec![]), ("parts".to_owned(), vec![])].into())
    );

    // 操作紀錄：明細記在主表那筆紀錄
    let logs = audit_logs(&pool, created.id).await;
    assert_eq!(logs.len(), 1);
    let (action, before, after) = &logs[0];
    assert_eq!(action, "create");
    assert_eq!(*before, None);
    assert_eq!(
        after.as_ref().unwrap(),
        &json!({
            "title": "冷氣維修",
            "lines.parts": audit_lines(lines(&created, "parts")),
            "lines.labor": audit_lines(lines(&created, "labor")),
        })
    );
    // 明細本身沒有另外的操作紀錄
    for line in lines(&created, "parts") {
        assert!(audit_logs(&pool, line.id).await.is_empty());
    }
}

/// 修改時新增、更新、刪除明細各一筆；沒帶到的子表不變；主表 `version` 遞增。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn update_replaces_listed_detail(pool: PgPool) {
    let token = setup(&pool, None).await;
    let created = create(
        &pool,
        &token,
        json!({
            "data": { "title": "冷氣維修" },
            "lines": {
                "parts": [
                    { "data": { "part": "濾網", "qty": 2 } },
                    { "data": { "part": "壓縮機", "qty": 1 } },
                ],
                "labor": [ { "data": { "task": "拆裝", "hours": "1.5" } } ],
            },
        }),
    )
    .await;
    let old = lines(&created, "parts");
    let (removed, kept) = (old[0].clone(), old[1].clone());

    let updated = update(
        &pool,
        &token,
        created.id,
        json!({
            "version": 1,
            "lines": {
                "parts": [
                    // 更新（只合併有帶到的欄位），並移到第一列
                    { "id": kept.id, "data": { "qty": 3 } },
                    // 新增
                    { "data": { "part": "冷媒", "qty": 1 } },
                ],
            },
        }),
    )
    .await;
    assert_eq!(updated.version, 2, "明細有變動時主表 version 遞增");
    assert_eq!(updated.data, created.data);
    let parts = lines(&updated, "parts");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].id, kept.id);
    assert_eq!(parts[0].data["part"], "壓縮機");
    assert_eq!(parts[0].data["qty"], 3);
    assert_eq!(parts[0].version, 2);
    assert_eq!(
        parts[1].data,
        json!({ "part": "冷媒", "qty": 1 })
            .as_object()
            .unwrap()
            .clone()
    );
    // 沒帶到的子表不變
    assert_eq!(lines(&updated, "labor"), lines(&created, "labor"));
    assert_eq!(get(&pool, &token, created.id).await, updated);

    // 被刪除的明細為軟刪除
    let (deleted,): (bool,) =
        sqlx::query_as("SELECT deleted_at IS NOT NULL FROM custom_records WHERE id = $1")
            .bind(removed.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deleted);

    // 操作紀錄只包含有變動的子表
    let logs = audit_logs(&pool, created.id).await;
    assert_eq!(logs.len(), 2);
    let (action, before, after) = &logs[1];
    assert_eq!(action, "update");
    assert_eq!(
        before.as_ref().unwrap(),
        &json!({ "lines.parts": audit_lines(lines(&created, "parts")) })
    );
    assert_eq!(
        after.as_ref().unwrap(),
        &json!({ "lines.parts": audit_lines(parts) })
    );

    // 內容相同（含順序）時不寫入，version 不變
    let same = update(
        &pool,
        &token,
        created.id,
        json!({
            "version": 2,
            "data": { "title": "冷氣維修" },
            "lines": {
                "parts": [ { "id": parts[0].id }, { "id": parts[1].id, "data": { "qty": 1 } } ],
            },
        }),
    )
    .await;
    assert_eq!(same, updated);
    assert_eq!(audit_logs(&pool, created.id).await.len(), 2);

    // 只調整順序也算變動
    let reordered = update(
        &pool,
        &token,
        created.id,
        json!({
            "version": 2,
            "lines": { "parts": [ { "id": parts[1].id }, { "id": parts[0].id } ] },
        }),
    )
    .await;
    assert_eq!(reordered.version, 3);
    assert_eq!(
        column(&reordered, "parts", "part"),
        [json!("冷媒"), json!("壓縮機")]
    );

    // 只修改主表時明細不變；空陣列刪除整個子表的明細
    let cleared = update(
        &pool,
        &token,
        created.id,
        json!({ "version": 3, "data": { "title": "改" }, "lines": { "labor": [] } }),
    )
    .await;
    assert_eq!(cleared.version, 4);
    assert_eq!(cleared.data["title"], "改");
    assert_eq!(lines(&cleared, "parts"), lines(&reordered, "parts"));
    assert!(lines(&cleared, "labor").is_empty());
}

/// 明細驗證錯誤時整筆不寫入，錯誤指出列號與欄位。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_lines_reject_whole_save(pool: PgPool) {
    let token = setup(&pool, None).await;

    // 新增：主表與明細的錯誤一起列出
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(MASTER),
        &token,
        Some(json!({
            "data": { "title": 1 },
            "lines": {
                "parts": [
                    { "data": { "part": "濾網", "qty": 2, "sn": "A" } },
                    { "data": { "qty": "abc" } },
                    { "data": { "part": "壓縮機", "sn": "A" } },
                ],
                "labor": [ { "data": { "hours": "1.55" } } ],
                "unknown": [ {} ],
            },
        })),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    let details = err.details.unwrap();
    assert_eq!(details["fields"][0]["field_key"], "title", "{body}");
    let errors = details["lines"].as_array().unwrap();
    let located: Vec<(&str, Option<u64>, Option<&str>)> = errors
        .iter()
        .map(|e| {
            (
                e["detail_key"].as_str().unwrap(),
                e["index"].as_u64(),
                e["field_key"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        located,
        [
            ("unknown", None, None),
            ("labor", Some(0), Some("hours")),
            ("parts", Some(1), Some("part")),
            ("parts", Some(1), Some("qty")),
            ("parts", Some(2), Some("sn")),
        ],
        "{body}"
    );
    assert!(
        errors
            .iter()
            .all(|e| !e["message"].as_str().unwrap().is_empty())
    );
    assert_eq!(record_counts(&pool).await, (0, 0), "整筆不寫入");

    // 只有明細錯誤：fields 為空
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(MASTER),
        &token,
        Some(json!({ "lines": { "parts": [ { "id": Id::nil(), "data": { "part": "x" } } ] } })),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details.as_ref().unwrap()["fields"], json!([]));
    assert_eq!(
        err.details.unwrap()["lines"],
        json!([{ "detail_key": "parts", "index": 0, "field_key": null,
                 "message": "新增資料時明細不可指定 id" }])
    );
    assert_eq!(record_counts(&pool).await, (0, 0));

    // 修改：任一列不合法，主表與其他明細都不變
    let created = create(
        &pool,
        &token,
        json!({ "data": { "title": "原" }, "lines": { "parts": [ { "data": { "part": "濾網" } } ] } }),
    )
    .await;
    let line_id = lines(&created, "parts")[0].id;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(created.id),
        &token,
        Some(json!({
            "version": 1,
            "data": { "title": "改" },
            "lines": {
                "parts": [
                    { "id": line_id, "data": { "qty": 1 } },
                    { "data": { "part": "冷媒", "qty": 1.5 } },
                ],
                "labor": [ { "data": { "task": "拆裝" } } ],
            },
        })),
    )
    .await;
    let errors = invalid_lines(status, &body);
    assert_eq!(errors.len(), 1, "{body}");
    assert_eq!(
        (
            &errors[0]["detail_key"],
            &errors[0]["index"],
            &errors[0]["field_key"]
        ),
        (&json!("parts"), &json!(1), &json!("qty")),
        "{body}"
    );
    assert_eq!(get(&pool, &token, created.id).await, created);
    assert_eq!(record_counts(&pool).await, (2, 2));

    // 修改：id 不屬於這筆資料、同一個 id 出現兩次
    let other = create(
        &pool,
        &token,
        json!({ "lines": { "parts": [ { "data": { "part": "其他" } } ] } }),
    )
    .await;
    let other_line = lines(&other, "parts")[0].id;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(created.id),
        &token,
        Some(json!({
            "version": 1,
            "lines": { "parts": [ { "id": line_id }, { "id": other_line }, { "id": line_id } ] },
        })),
    )
    .await;
    let errors = invalid_lines(status, &body);
    let indexes: Vec<u64> = errors
        .iter()
        .map(|e| e["index"].as_u64().unwrap())
        .collect();
    assert_eq!(indexes, [1, 2], "{body}");
    assert_eq!(get(&pool, &token, created.id).await, created);
    assert_eq!(get(&pool, &token, other.id).await, other);

    // 與其他主表的明細重複「不可重複」的值
    update(
        &pool,
        &token,
        other.id,
        json!({ "version": 1, "lines": { "parts": [ { "id": other_line, "data": { "sn": "S-1" } } ] } }),
    )
    .await;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(created.id),
        &token,
        Some(json!({
            "version": 1,
            "lines": { "parts": [ { "id": line_id }, { "data": { "part": "新", "sn": "S-1" } } ] },
        })),
    )
    .await;
    assert_eq!(
        invalid_lines(status, &body),
        [
            json!({ "detail_key": "parts", "index": 1, "field_key": "sn",
                 "message": "此值已被其他資料使用" })
        ],
        "{body}"
    );
    assert_eq!(get(&pool, &token, created.id).await, created);
}

/// 主表 `version` 不符回傳 409，明細不變。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn stale_version_returns_conflict(pool: PgPool) {
    let token = setup(&pool, None).await;
    let created = create(
        &pool,
        &token,
        json!({ "lines": { "parts": [ { "data": { "part": "濾網" } } ] } }),
    )
    .await;
    let line_id = lines(&created, "parts")[0].id;
    // 其他人只修改了明細，主表的 version 也遞增
    let updated = update(
        &pool,
        &token,
        created.id,
        json!({ "version": 1, "lines": { "parts": [ { "id": line_id, "data": { "qty": 2 } } ] } }),
    )
    .await;
    assert_eq!(updated.version, 2);

    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(created.id),
        &token,
        Some(json!({ "version": 1, "lines": { "parts": [] } })),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    assert_eq!(get(&pool, &token, created.id).await, updated);

    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri(created.id)),
        &token,
        None,
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    assert_eq!(record_counts(&pool).await, (2, 2));
}

/// 明細子表不能透過通用端點直接存取；定義與欄位 API 可以使用。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn detail_records_endpoints_return_not_found(pool: PgPool) {
    let token = setup(&pool, None).await;
    let created = create(
        &pool,
        &token,
        json!({ "lines": { "parts": [ { "data": { "part": "濾網" } } ] } }),
    )
    .await;
    let line = &lines(&created, "parts")[0];

    let uri = records_uri("parts");
    let line_uri = format!("{uri}/{}", line.id);
    for (method, uri, body) in [
        (Method::GET, uri.clone(), None),
        (Method::GET, format!("{uri}/lookup"), None),
        (
            Method::POST,
            uri.clone(),
            Some(json!({ "data": { "part": "x" } })),
        ),
        (Method::GET, line_uri.clone(), None),
        (
            Method::PATCH,
            line_uri.clone(),
            Some(json!({ "version": 1, "data": { "part": "x" } })),
        ),
        (Method::DELETE, format!("{line_uri}?version=1"), None),
    ] {
        let (status, body) = call(&pool, method.clone(), &uri, &token, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}：{body}");
    }
    assert_eq!(get(&pool, &token, created.id).await, created);

    // 資料類別定義可以讀取（含主表）
    let (status, body) = call(&pool, Method::GET, "/api/v1/entities/parts", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    assert_eq!(detail.fields.len(), 3);
}

/// 主表刪除後明細也被刪除（軟刪除），操作紀錄包含明細。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn deleting_master_deletes_lines(pool: PgPool) {
    let token = setup(&pool, None).await;
    let created = create(
        &pool,
        &token,
        json!({
            "data": { "title": "冷氣維修" },
            "lines": {
                "parts": [ { "data": { "part": "濾網" } }, { "data": { "part": "冷媒" } } ],
                "labor": [ { "data": { "task": "拆裝" } } ],
            },
        }),
    )
    .await;
    let other = create(
        &pool,
        &token,
        json!({ "lines": { "parts": [ { "data": { "part": "其他" } } ] } }),
    )
    .await;
    assert_eq!(record_counts(&pool).await, (6, 6));

    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri(created.id)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        record_counts(&pool).await,
        (6, 2),
        "主表與 3 筆明細都被軟刪除"
    );
    let (status, _) = call(&pool, Method::GET, &record_uri(created.id), &token, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 其他主表的明細不受影響
    assert_eq!(get(&pool, &token, other.id).await, other);

    let logs = audit_logs(&pool, created.id).await;
    let (action, before, after) = logs.last().unwrap();
    assert_eq!(action, "delete");
    assert_eq!(*after, None);
    assert_eq!(
        before.as_ref().unwrap(),
        &json!({
            "title": "冷氣維修",
            "lines.parts": audit_lines(lines(&created, "parts")),
            "lines.labor": audit_lines(lines(&created, "labor")),
        })
    );
}

/// 有單號規則的單據新增時取得單號；明細沒有單號。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn numbered_master_gets_record_no(pool: PgPool) {
    let token = setup(&pool, Some("RO-{000}")).await;
    let first = create(
        &pool,
        &token,
        json!({ "lines": { "parts": [ { "data": { "part": "濾網" } } ] } }),
    )
    .await;
    assert_eq!(first.record_no.as_deref(), Some("RO-001"));
    assert_eq!(lines(&first, "parts")[0].record_no, None);
    let second = create(
        &pool,
        &token,
        json!({ "lines": { "labor": [ { "data": { "task": "檢查" } } ] } }),
    )
    .await;
    assert_eq!(second.record_no.as_deref(), Some("RO-002"));

    // 明細驗證失敗時不消耗號碼
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(MASTER),
        &token,
        Some(json!({ "lines": { "parts": [ { "data": {} } ] } })),
    )
    .await;
    assert_eq!(invalid_lines(status, &body).len(), 1);
    let third = create(&pool, &token, json!({})).await;
    assert_eq!(third.record_no.as_deref(), Some("RO-003"));

    // 操作紀錄包含單號與明細
    let logs = audit_logs(&pool, first.id).await;
    let after = logs[0].2.as_ref().unwrap();
    assert_eq!(after["record_no"], "RO-001");
    assert_eq!(after["lines.parts"], audit_lines(lines(&first, "parts")));
}

/// 建立明細子表的規則：必須指定主表、主表不可是明細子表、不可設定單號規則、不可更換主表。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn detail_entity_rules(pool: PgPool) {
    let token = setup(&pool, None).await;
    let (status, body) = call(&pool, Method::GET, "/api/v1/entities/parts", &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let parts: EntityDetail = serde_json::from_value(body).unwrap();
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{MASTER}"),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let master: EntityDetail = serde_json::from_value(body).unwrap();

    let post = |body: Value| {
        let pool = pool.clone();
        let token = token.clone();
        async move { call(&pool, Method::POST, "/api/v1/entities", &token, Some(body)).await }
    };
    // 沒有指定主表
    let (status, body) = post(json!({ "key": "a", "label": "A", "kind": "detail" })).await;
    assert_invalid_field(status, &body, "parent_entity_id");
    // 主表不存在
    let (status, body) = post(json!({
        "key": "a", "label": "A", "kind": "detail", "parent_entity_id": Id::nil(),
    }))
    .await;
    assert_invalid_field(status, &body, "parent_entity_id");
    // 主表是另一個明細子表（不支援多層明細）
    let (status, body) = post(json!({
        "key": "a", "label": "A", "kind": "detail", "parent_entity_id": parts.entity.id,
    }))
    .await;
    assert_invalid_field(status, &body, "parent_entity_id");
    // 明細子表不可設定單號規則
    let (status, body) = post(json!({
        "key": "a", "label": "A", "kind": "detail", "parent_entity_id": master.entity.id,
        "numbering_pattern": "A-{000}",
    }))
    .await;
    assert_invalid_field(status, &body, "numbering_pattern");
    // 主檔不可指定主表
    let (status, body) = post(json!({
        "key": "a", "label": "A", "kind": "master", "parent_entity_id": master.entity.id,
    }))
    .await;
    assert_invalid_field(status, &body, "parent_entity_id");

    // 修改明細子表：不可設定單號規則；request 中的主表被忽略（不可更換）
    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/parts",
        &token,
        Some(json!({
            "version": parts.entity.version, "label": "零件", "icon": null,
            "numbering_pattern": "P-{000}", "title_template": null, "sort_order": 0,
            "is_active": true,
        })),
    )
    .await;
    assert_invalid_field(status, &body, "numbering_pattern");
    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/parts",
        &token,
        Some(json!({
            "version": parts.entity.version, "label": "零件", "icon": null,
            "numbering_pattern": null, "title_template": "{part}", "sort_order": 0,
            "is_active": true, "parent_entity_id": Id::nil(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: EntityDetail = serde_json::from_value(body).unwrap();
    assert_eq!(updated.entity.label, "零件");
    assert_eq!(updated.entity.parent_entity_id, Some(master.entity.id));

    // 停用的明細子表：不出現在回應中，request 帶到時視為不存在
    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/entities/labor",
        &token,
        Some(json!({
            "version": 1, "label": "工時", "icon": null, "numbering_pattern": null,
            "title_template": null, "sort_order": 0, "is_active": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let created = create(
        &pool,
        &token,
        json!({ "lines": { "parts": [ { "data": { "part": "濾網" } } ] } }),
    )
    .await;
    let keys: Vec<&String> = created.lines.as_ref().unwrap().keys().collect();
    assert_eq!(keys, ["parts"]);
    assert_eq!(lines(&created, "parts")[0].title, "濾網");
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(MASTER),
        &token,
        Some(json!({ "lines": { "labor": [] } })),
    )
    .await;
    assert_eq!(
        invalid_lines(status, &body),
        [
            json!({ "detail_key": "labor", "index": null, "field_key": null,
                 "message": "沒有此明細子表" })
        ]
    );
}
