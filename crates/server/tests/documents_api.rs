//! 系統單據共通框架的整合測試（#129，docs/architecture.md §7.1「系統單據」、§8.4、§10「單據動作」）：
//! 以盤點調整單 `stock_adjustment` 驗證單號、系統維護的欄位、只有草稿可以修改與刪除、過帳與作廢端點
//! （權限、`version`、狀態、操作紀錄、處理函式失敗時 rollback）。
//!
//! 過帳的庫存處理在盤點調整單的 issue 實作，這裡使用不做事（或測試用）的處理函式。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::CoreError;
use hammer_core::documents::{DocumentContext, DocumentHandler, DocumentHandlers, HandlerFuture};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::document::DocumentInfo;
use hammer_shared::record::Record;
use hammer_shared::schema::EntityDetail;
use hammer_shared::time::now_taipei;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use tower::ServiceExt;

const DOC: &str = "stock_adjustment";
const LINE: &str = "stock_adjustment_line";

fn state(pool: &PgPool) -> AppState {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    AppState::new(pool.clone(), config)
}

async fn call_with(
    app: Router,
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
    let res = app.oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

async fn call(
    pool: &PgPool,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call_with(router(state(pool)), method, uri, token, body).await
}

async fn token_for(pool: &PgPool, username: &str, role_code: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role_code))
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

fn action_uri(entity: &str, id: Id, action: &str) -> String {
    format!("/api/v1/entities/{entity}/records/{id}/{action}")
}

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get_record(pool: &PgPool, token: &str, id: Id) -> Record {
    let (status, body) = call(pool, Method::GET, &record_uri(DOC, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 新增一張盤點調整單草稿（沒有明細）。
async fn draft(pool: &PgPool, token: &str) -> Record {
    create(
        pool,
        token,
        DOC,
        json!({ "data": { "adjustment_date": "2026-11-17", "notes": "月底盤點" } }),
    )
    .await
}

async fn act(
    pool: &PgPool,
    token: &str,
    id: Id,
    action: &str,
    version: i32,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &action_uri(DOC, id, action),
        token,
        Some(json!({ "version": version })),
    )
    .await
}

async fn act_ok(pool: &PgPool, token: &str, record: &Record, action: &str) -> Record {
    let (status, body) = act(pool, token, record.id, action, record.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

fn error(status: StatusCode, body: &Value, expected: StatusCode, code: ErrorCode) -> ApiError {
    assert_eq!(status, expected, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, code, "{body}");
    err
}

/// `422 INVALID_STATUS`，`details.status` 為 `status`；回傳訊息。
fn assert_invalid_status(status: StatusCode, body: &Value, current: &str) -> String {
    let err = error(
        status,
        body,
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::InvalidStatus,
    );
    assert_eq!(err.details, Some(json!({ "status": current })), "{body}");
    err.message
}

/// `422 VALIDATION`，`details.fields` 的 `field_key` 為 `keys`。
fn assert_invalid_fields(status: StatusCode, body: &Value, keys: &[&str]) {
    let err = error(
        status,
        body,
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
    );
    let details = err.details.unwrap();
    let actual: Vec<&str> = details["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field_key"].as_str().unwrap())
        .collect();
    assert_eq!(actual, keys, "{body}");
}

/// 目前 Asia/Taipei 的 `yyyyMM`（單號的期間）。
fn period() -> String {
    let now = now_taipei();
    format!("{:04}{:02}", now.year(), u8::from(now.month()))
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

async fn product(pool: &PgPool, token: &str) -> Id {
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await;
    create(
        pool,
        token,
        "product",
        json!({ "data": { "code": "A001", "name": "螺絲", "unit_id": unit.id } }),
    )
    .await
    .id
}

// ---------------------------------------------------------------------------
// 單號與系統維護的欄位
// ---------------------------------------------------------------------------

/// 新增時依預設規則 `ADJ{yyyyMM}-{0000}` 取號、狀態為草稿；request 帶的 `record_no` 與 `status` 忽略。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn create_generates_record_no_and_draft_status(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    let first = create(
        &pool,
        &token,
        DOC,
        json!({
            "record_no": "HACK-001",
            "data": { "adjustment_date": "2026-11-17", "status": "posted" },
        }),
    )
    .await;
    let period = period();
    assert_eq!(
        first.record_no.as_deref(),
        Some(format!("ADJ{period}-0001").as_str())
    );
    // 標題範本 `{record_no}`
    assert_eq!(first.title, format!("ADJ{period}-0001"));
    assert_eq!(first.data["status"], json!("draft"));
    assert_eq!(first.data["adjustment_type"], json!("count"));
    assert_eq!(first.document, Some(DocumentInfo::default()));
    assert_eq!(
        first.lines,
        Some([(LINE.to_owned(), Vec::new())].into_iter().collect())
    );

    let second = draft(&pool, &token).await;
    assert_eq!(
        second.record_no.as_deref(),
        Some(format!("ADJ{period}-0002").as_str())
    );

    // 單筆查詢的回應也有 `document`（JSON 為四個 null）
    let (status, body) = call(&pool, Method::GET, &record_uri(DOC, first.id), &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["document"],
        json!({ "posted_at": null, "posted_by": null, "voided_at": null, "voided_by": null })
    );
    assert_eq!(body["record_no"], json!(format!("ADJ{period}-0001")));

    // 操作紀錄包含單號與狀態
    let logs = audit_logs(&pool, first.id).await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].action, "create");
    let after = logs[0].after_data.as_ref().unwrap();
    assert_eq!(after["record_no"], json!(format!("ADJ{period}-0001")));
    assert_eq!(after["status"], json!("draft"));
}

/// `data` 帶 `record_no` 或不是欄位的鍵（`posted_at`、`lot_id`）→ 422，不消耗單號。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn non_field_keys_are_rejected(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    for key in ["record_no", "posted_at", "voided_by"] {
        let (status, body) = call(
            &pool,
            Method::POST,
            &records_uri(DOC),
            &token,
            Some(json!({ "data": { "adjustment_date": "2026-11-17", key: "x" } })),
        )
        .await;
        assert_invalid_fields(status, &body, &[key]);
    }
    let product = product(&pool, &token).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(DOC),
        &token,
        Some(json!({
            "data": { "adjustment_date": "2026-11-17" },
            "lines": { LINE: [{ "data": {
                "product_id": product, "qty": "1", "unit_cost_twd": "10", "lot_id": product,
            } }] },
        })),
    )
    .await;
    let err = error(
        status,
        &body,
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
    );
    assert_eq!(
        err.details.unwrap()["lines"][0]["field_key"],
        json!("lot_id")
    );

    let record = draft(&pool, &token).await;
    assert_eq!(
        record.record_no.as_deref(),
        Some(format!("ADJ{}-0001", period()).as_str())
    );
}

/// 修改草稿時帶 `status` 忽略；主表與明細可以一起修改。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn draft_can_be_updated_and_status_is_ignored(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    let product = product(&pool, &token).await;
    let record = create(
        &pool,
        &token,
        DOC,
        json!({
            "data": { "adjustment_date": "2026-11-17" },
            "lines": { LINE: [{ "data": { "product_id": product, "qty": "5", "unit_cost_twd": "12.5" } }] },
        }),
    )
    .await;
    let lines = &record.lines.as_ref().unwrap()[LINE];
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].data["qty"], json!("5.0000"));
    assert_eq!(lines[0].data["line_type"], json!("gain_new"));
    assert_eq!(lines[0].record_no, None);
    assert_eq!(lines[0].document, None);

    // 只帶 `status`：視為沒有變動
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(DOC, record.id),
        &token,
        Some(json!({ "version": record.version, "data": { "status": "void" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let same: Record = serde_json::from_value(body).unwrap();
    assert_eq!(same.version, record.version);
    assert_eq!(same.data["status"], json!("draft"));

    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(DOC, record.id),
        &token,
        Some(json!({
            "version": record.version,
            "data": { "notes": "改", "status": "posted" },
            "lines": { LINE: [{ "id": lines[0].id, "data": { "qty": "6" } }] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: Record = serde_json::from_value(body).unwrap();
    assert_eq!(updated.version, record.version + 1);
    assert_eq!(updated.data["notes"], json!("改"));
    assert_eq!(updated.data["status"], json!("draft"));
    assert_eq!(updated.record_no, record.record_no);
    assert_eq!(
        updated.lines.as_ref().unwrap()[LINE][0].data["qty"],
        json!("6.0000")
    );
}

/// 列表可依 `record_no` 排序、篩選（`eq`、`contains`），也可依 `status` 篩選；列表回應有 `document`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_sorts_and_filters_by_record_no_and_status(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    let a = draft(&pool, &token).await;
    let b = draft(&pool, &token).await;
    let c = draft(&pool, &token).await;
    act_ok(&pool, &token, &b, "post").await;

    let list = |query: String| {
        let pool = pool.clone();
        let token = token.clone();
        async move {
            let (status, body) = call(
                &pool,
                Method::GET,
                &format!("{}?{query}", records_uri(DOC)),
                &token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body
        }
    };
    let ids = |body: &Value| -> Vec<Id> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| serde_json::from_value(r["id"].clone()).unwrap())
            .collect()
    };

    let body = list("sort=record_no".into()).await;
    assert_eq!(ids(&body), [a.id, b.id, c.id]);
    assert_eq!(body["items"][1]["data"]["status"], json!("posted"));
    assert!(body["items"][1]["document"]["posted_at"].is_string());
    assert_eq!(body["items"][0]["document"]["posted_at"], json!(null));
    assert_eq!(
        ids(&list("sort=-record_no".into()).await),
        [c.id, b.id, a.id]
    );
    let no = b.record_no.clone().unwrap();
    assert_eq!(
        ids(&list(format!("filter.record_no.eq={no}")).await),
        [b.id]
    );
    assert_eq!(
        ids(&list("filter.record_no.contains=-0003".into()).await),
        [c.id]
    );
    assert_eq!(
        ids(&list("filter.status.eq=draft&sort=record_no".into()).await),
        [a.id, c.id]
    );
}

/// 刪除草稿後單號不再使用。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn deleted_record_no_is_not_reused(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    let first = draft(&pool, &token).await;
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version={}", record_uri(DOC, first.id), first.version),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let second = draft(&pool, &token).await;
    assert_eq!(
        second.record_no.as_deref(),
        Some(format!("ADJ{}-0002", period()).as_str())
    );
}

async fn entity(pool: &PgPool, token: &str, key: &str) -> EntityDetail {
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

async fn patch_numbering(
    pool: &PgPool,
    token: &str,
    key: &str,
    pattern: Option<&str>,
) -> (StatusCode, Value) {
    let e = entity(pool, token, key).await.entity;
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/entities/{key}"),
        token,
        Some(json!({
            "version": e.version,
            "label": e.label,
            "icon": e.icon,
            "numbering_pattern": pattern,
            "title_template": e.title_template,
            "sort_order": e.sort_order,
            "is_active": e.is_active,
        })),
    )
    .await
}

/// 系統單據的單號規則可以修改、不可清空；主檔類的系統資料類別不可設定單號規則。
/// 修改後產生的單號與既有的單號重複時新增失敗（422）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn numbering_pattern_rules_for_system_entities(pool: PgPool) {
    let admin = token_for(&pool, "admin", role::ADMIN).await;
    for pattern in [None, Some(" ")] {
        let (status, body) = patch_numbering(&pool, &admin, DOC, pattern).await;
        let err = error(
            status,
            &body,
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::Validation,
        );
        assert_eq!(err.details, Some(json!({ "field": "numbering_pattern" })));
    }
    let (status, body) = patch_numbering(&pool, &admin, "customer", Some("C-{000}")).await;
    let err = error(
        status,
        &body,
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
    );
    assert_eq!(err.details, Some(json!({ "field": "numbering_pattern" })));

    let first = draft(&pool, &admin).await;
    // 沒有日期代碼、文字與既有單號相同 → 流水號從 1 開始，與第一張重複
    let literal = format!("ADJ{}-{{0000}}", period());
    let (status, body) = patch_numbering(&pool, &admin, DOC, Some(&literal)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(DOC),
        &admin,
        Some(json!({ "data": { "adjustment_date": "2026-11-17" } })),
    )
    .await;
    let err = error(
        status,
        &body,
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
    );
    assert!(err.message.contains("單號"), "{body}");

    let (status, body) = patch_numbering(&pool, &admin, DOC, Some("SA-{000}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let next = draft(&pool, &admin).await;
    assert_eq!(next.record_no.as_deref(), Some("SA-001"));
    assert_ne!(first.record_no, next.record_no);
}

// ---------------------------------------------------------------------------
// 過帳與作廢
// ---------------------------------------------------------------------------

/// 過帳：狀態、`document`、`version`、操作紀錄；之後不可修改、刪除、再過帳。作廢同理。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn post_then_void(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    let product = product(&pool, &token).await;
    let record = create(
        &pool,
        &token,
        DOC,
        json!({
            "data": { "adjustment_date": "2026-11-17" },
            "lines": { LINE: [{ "data": { "product_id": product, "qty": "5", "unit_cost_twd": "12" } }] },
        }),
    )
    .await;

    // 草稿不可作廢
    let (status, body) = act(&pool, &token, record.id, "void", record.version).await;
    assert_eq!(
        assert_invalid_status(status, &body, "draft"),
        "草稿的單據不可作廢"
    );

    let posted = act_ok(&pool, &token, &record, "post").await;
    assert_eq!(posted.version, record.version + 1);
    assert_eq!(posted.data["status"], json!("posted"));
    assert_eq!(posted.record_no, record.record_no);
    assert_eq!(posted.lines.as_ref().unwrap()[LINE].len(), 1);
    let info = posted.document.clone().unwrap();
    assert!(info.posted_at.is_some());
    assert_eq!(info.posted_by, posted.updated_by);
    assert_eq!((info.voided_at, info.voided_by), (None, None));
    assert_eq!(get_record(&pool, &token, record.id).await, posted);

    // 已過帳：不可再過帳、修改、刪除
    let (status, body) = act(&pool, &token, record.id, "post", posted.version).await;
    assert_eq!(
        assert_invalid_status(status, &body, "posted"),
        "已過帳的單據不可過帳"
    );
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(DOC, record.id),
        &token,
        Some(json!({ "version": posted.version, "data": { "notes": "x" } })),
    )
    .await;
    assert_eq!(
        assert_invalid_status(status, &body, "posted"),
        "已過帳的單據不可修改"
    );
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version={}", record_uri(DOC, record.id), posted.version),
        &token,
        None,
    )
    .await;
    assert_eq!(
        assert_invalid_status(status, &body, "posted"),
        "已過帳的單據不可刪除"
    );

    let voided = act_ok(&pool, &token, &posted, "void").await;
    assert_eq!(voided.version, posted.version + 1);
    assert_eq!(voided.data["status"], json!("void"));
    let info = voided.document.clone().unwrap();
    assert_eq!(info.posted_at, posted.document.as_ref().unwrap().posted_at);
    assert!(info.voided_at.is_some());
    assert!(info.voided_by.is_some());

    let (status, body) = act(&pool, &token, record.id, "void", voided.version).await;
    assert_eq!(
        assert_invalid_status(status, &body, "void"),
        "已作廢的單據不可作廢"
    );
    let (status, body) = act(&pool, &token, record.id, "post", voided.version).await;
    assert_invalid_status(status, &body, "void");
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(DOC, record.id),
        &token,
        Some(json!({ "version": voided.version, "lines": { LINE: [] } })),
    )
    .await;
    assert_eq!(
        assert_invalid_status(status, &body, "void"),
        "已作廢的單據不可修改"
    );
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version={}", record_uri(DOC, record.id), voided.version),
        &token,
        None,
    )
    .await;
    assert_eq!(
        assert_invalid_status(status, &body, "void"),
        "已作廢的單據不可刪除"
    );

    // 操作紀錄：create、post、void；diff 只有 status
    let logs = audit_logs(&pool, record.id).await;
    let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
    assert_eq!(actions, ["create", "post", "void"]);
    for log in &logs {
        assert_eq!(log.entity_key.as_deref(), Some(DOC));
    }
    assert_eq!(logs[1].before_data, Some(json!({ "status": "draft" })));
    assert_eq!(logs[1].after_data, Some(json!({ "status": "posted" })));
    assert_eq!(logs[2].before_data, Some(json!({ "status": "posted" })));
    assert_eq!(logs[2].after_data, Some(json!({ "status": "void" })));
}

/// 檢查順序：權限（403）→ 資料存在（404）→ `version`（409）→ 狀態（422）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn action_checks_permission_existence_version_then_status(pool: PgPool) {
    let editor = token_for(&pool, "editor", role::EDITOR).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let record = draft(&pool, &editor).await;

    for action in ["post", "void"] {
        // 唯讀者沒有 record.post，即使資料不存在也是 403
        let (status, body) = act(&pool, &viewer, record.id, action, record.version).await;
        let err = error(status, &body, StatusCode::FORBIDDEN, ErrorCode::Forbidden);
        assert_eq!(err.details, Some(json!({ "permission": "record.post" })));
        let (status, body) = act(&pool, &viewer, hammer_shared::new_id(), action, 1).await;
        error(status, &body, StatusCode::FORBIDDEN, ErrorCode::Forbidden);

        let (status, body) = act(&pool, &editor, hammer_shared::new_id(), action, 1).await;
        error(status, &body, StatusCode::NOT_FOUND, ErrorCode::NotFound);

        // version 不符優先於狀態（草稿作廢）
        let (status, body) = act(&pool, &editor, record.id, action, record.version + 1).await;
        error(status, &body, StatusCode::CONFLICT, ErrorCode::Conflict);
    }

    // 修改、刪除非草稿：version 不符時為 409
    let posted = act_ok(&pool, &editor, &record, "post").await;
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(DOC, record.id),
        &editor,
        Some(json!({ "version": record.version, "data": { "notes": "x" } })),
    )
    .await;
    error(status, &body, StatusCode::CONFLICT, ErrorCode::Conflict);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version={}", record_uri(DOC, record.id), record.version),
        &editor,
        None,
    )
    .await;
    error(status, &body, StatusCode::CONFLICT, ErrorCode::Conflict);

    // 狀態與版本都沒有被改變
    let current = get_record(&pool, &editor, record.id).await;
    assert_eq!(current, posted);
    let actions: Vec<String> = audit_logs(&pool, record.id)
        .await
        .into_iter()
        .map(|l| l.action)
        .collect();
    assert_eq!(actions, ["create", "post"]);
}

/// 不支援過帳的資料類別（主檔、明細子表、自訂資料類別、不存在的代碼）→ 404。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn unsupported_entities_are_not_found(pool: PgPool) {
    let admin = token_for(&pool, "admin", role::ADMIN).await;
    let customer = create(
        &pool,
        &admin,
        "customer",
        json!({ "data": { "code": "C001", "name": "客戶" } }),
    )
    .await;
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/entities",
        &admin,
        Some(json!({ "key": "repair_order", "label": "維修單", "kind": "document" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let custom = create(&pool, &admin, "repair_order", json!({ "data": {} })).await;

    for (entity, id) in [
        ("customer", customer.id),
        (LINE, customer.id),
        ("repair_order", custom.id),
        ("no_such_entity", customer.id),
    ] {
        for action in ["post", "void"] {
            let (status, body) = call(
                &pool,
                Method::POST,
                &action_uri(entity, id, action),
                &admin,
                Some(json!({ "version": 1 })),
            )
            .await;
            error(status, &body, StatusCode::NOT_FOUND, ErrorCode::NotFound);
        }
    }
    // 自訂資料類別的回應沒有 document
    assert_eq!(custom.document, None);
    assert_eq!(customer.document, None);
}

/// request 只接受 `version`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn action_request_accepts_only_version(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    let record = draft(&pool, &token).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &action_uri(DOC, record.id, "post"),
        &token,
        Some(json!({ "version": record.version, "data": { "notes": "x" } })),
    )
    .await;
    assert!(status.is_client_error(), "{status} {body}");
    assert_ne!(status, StatusCode::OK);
    let current = get_record(&pool, &token, record.id).await;
    assert_eq!(current.data["status"], json!("draft"));
}

// ---------------------------------------------------------------------------
// 處理函式
// ---------------------------------------------------------------------------

/// 寫入 `notes` 的處理函式；`fail` 為 true 時寫入後回傳錯誤。
struct WriteNotes {
    fail: bool,
}

impl WriteNotes {
    fn run<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
        note: &'static str,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            sqlx::query("UPDATE stock_adjustments SET notes = $1 WHERE id = $2")
                .bind(format!("{note} {}", doc.record_no))
                .bind(doc.record_id)
                .execute(&mut *conn)
                .await
                .map_err(|e| CoreError::Internal(e.to_string()))?;
            assert_eq!(doc.entity.key, DOC);
            assert_eq!(
                doc.data["status"],
                json!(if note == "post" { "draft" } else { "posted" })
            );
            if self.fail {
                Err(CoreError::Validation("測試用的過帳失敗".into()))
            } else {
                Ok(())
            }
        })
    }
}

impl DocumentHandler for WriteNotes {
    fn post<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        self.run(conn, doc, "post")
    }

    fn void<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        self.run(conn, doc, "void")
    }
}

fn app_with(pool: &PgPool, handler: WriteNotes) -> Router {
    let mut state = state(pool);
    state.documents = DocumentHandlers::empty().register(DOC, handler);
    router(state)
}

/// 處理函式寫入的值記在操作紀錄的 diff。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn handler_changes_are_audited(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    let record = draft(&pool, &token).await;
    let no = record.record_no.clone().unwrap();
    let (status, body) = call_with(
        app_with(&pool, WriteNotes { fail: false }),
        Method::POST,
        &action_uri(DOC, record.id, "post"),
        &token,
        Some(json!({ "version": record.version })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let posted: Record = serde_json::from_value(body).unwrap();
    assert_eq!(posted.data["notes"], json!(format!("post {no}")));

    let logs = audit_logs(&pool, record.id).await;
    assert_eq!(logs[1].action, "post");
    assert_eq!(
        logs[1].before_data,
        Some(json!({ "status": "draft", "notes": "月底盤點" }))
    );
    assert_eq!(
        logs[1].after_data,
        Some(json!({ "status": "posted", "notes": format!("post {no}") }))
    );
}

/// 處理函式失敗時整張 rollback：狀態、版本、處理函式寫入的值都不變，也沒有操作紀錄。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn handler_failure_rolls_back(pool: PgPool) {
    let token = token_for(&pool, "editor", role::EDITOR).await;
    let record = draft(&pool, &token).await;
    let (status, body) = call_with(
        app_with(&pool, WriteNotes { fail: true }),
        Method::POST,
        &action_uri(DOC, record.id, "post"),
        &token,
        Some(json!({ "version": record.version })),
    )
    .await;
    let err = error(
        status,
        &body,
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
    );
    assert_eq!(err.message, "測試用的過帳失敗");
    assert_eq!(get_record(&pool, &token, record.id).await, record);
    assert_eq!(audit_logs(&pool, record.id).await.len(), 1);

    // 作廢失敗同理
    let posted = act_ok(&pool, &token, &record, "post").await;
    let (status, body) = call_with(
        app_with(&pool, WriteNotes { fail: true }),
        Method::POST,
        &action_uri(DOC, record.id, "void"),
        &token,
        Some(json!({ "version": posted.version })),
    )
    .await;
    error(
        status,
        &body,
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Validation,
    );
    assert_eq!(get_record(&pool, &token, record.id).await, posted);
}
