//! 單號規則的 API 整合測試（#68）：資料類別設定 `numbering_pattern`、通用資料 API 產生
//! `record_no`、客戶端帶的單號被忽略、列表依單號排序與篩選。
//!
//! 取號的細節（跨月、並行、修改規則）在 `crates/core/tests/numbering.rs`，可以指定建立時間。
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
use hammer_shared::time::today_taipei;
use hammer_shared::{ApiError, ErrorCode, role};
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

/// 422 VALIDATION，且 `details.field` 為 `field`。
fn assert_invalid_field(status: StatusCode, body: &Value, field: &str) {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert_eq!(err.details.unwrap()["field"], field, "{body}");
}

async fn admin_token(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    token.into_string()
}

/// 建立資料類別 [`ENTITY`]（單號規則 `pattern`）與文字欄位 `title`。
async fn setup(pool: &PgPool, token: &str, pattern: Value) -> EntityDetail {
    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        token,
        Some(json!({
            "key": ENTITY, "label": "維修單", "kind": "master", "numbering_pattern": pattern,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, field) = call(
        pool,
        Method::POST,
        &format!("/api/v1/entities/{ENTITY}/fields"),
        token,
        Some(json!({ "field_key": "title", "label": "標題", "data_type": "text" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{field}");
    serde_json::from_value(body).unwrap()
}

fn records_uri() -> String {
    format!("/api/v1/entities/{ENTITY}/records")
}

async fn create(pool: &PgPool, token: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn list(pool: &PgPool, token: &str, query: &str) -> Vec<String> {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("{}?{query}", records_uri()),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    page.items
        .into_iter()
        .map(|r| r.record_no.unwrap_or_default())
        .collect()
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn entity_api_sets_validates_and_clears_pattern(pool: PgPool) {
    let token = admin_token(&pool).await;

    // 不合法的規則
    for pattern in [
        "RO-",
        "RO-{000}{000}",
        "RO-{00}",
        "RO-{HH}{000}",
        "RO-{yyyy",
    ] {
        let (status, body) = call(
            &pool,
            Method::POST,
            "/api/v1/entities",
            &token,
            Some(json!({
                "key": "bad", "label": "錯誤", "kind": "master", "numbering_pattern": pattern,
            })),
        )
        .await;
        assert_invalid_field(status, &body, "numbering_pattern");
    }

    // 前後空白去除
    let detail = setup(&pool, &token, json!(" RO{yyyyMM}-{000} ")).await;
    assert_eq!(
        detail.entity.numbering_pattern.as_deref(),
        Some("RO{yyyyMM}-{000}")
    );

    let update = |version: i32, pattern: Value| {
        json!({
            "version": version, "label": "維修單", "icon": null, "numbering_pattern": pattern,
            "title_template": null, "sort_order": 0, "is_active": true,
        })
    };
    let uri = format!("/api/v1/entities/{ENTITY}");
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(update(detail.entity.version, json!("RO-{000}{0000}"))),
    )
    .await;
    assert_invalid_field(status, &body, "numbering_pattern");

    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(update(detail.entity.version, json!("WO-{0000}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    assert_eq!(
        detail.entity.numbering_pattern.as_deref(),
        Some("WO-{0000}")
    );
    assert_eq!(
        create(&pool, &token, json!({ "data": {} }))
            .await
            .record_no
            .as_deref(),
        Some("WO-0001")
    );

    // 清除規則
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &uri,
        &token,
        Some(update(detail.entity.version, json!(null))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    assert_eq!(detail.entity.numbering_pattern, None);
    assert_eq!(
        create(&pool, &token, json!({ "data": {} })).await.record_no,
        None
    );

    // 規則的變更寫入 schema_change_logs
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM schema_change_logs
         WHERE target_type = 'entity' AND action = 'update'
           AND after_data ? 'numbering_pattern'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(logged, 2);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn client_record_no_is_ignored(pool: PgPool) {
    let token = admin_token(&pool).await;
    setup(&pool, &token, json!("RO{yyyyMM}-{000}")).await;
    // 測試剛好跨月時可能失敗，機率可忽略
    let today = today_taipei();
    let period = format!("{}{:02}", today.year(), u8::from(today.month()));

    // 新增時帶 record_no：忽略，由伺服器產生
    let record = create(
        &pool,
        &token,
        json!({ "record_no": "HACK-999", "data": { "title": "A" } }),
    )
    .await;
    let expected = format!("RO{period}-001");
    assert_eq!(record.record_no.as_deref(), Some(expected.as_str()));

    // 修改時帶 record_no：忽略，單號不變
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &format!("{}/{}", records_uri(), record.id),
        &token,
        Some(
            json!({ "version": record.version, "record_no": "HACK-999", "data": { "title": "B" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: Record = serde_json::from_value(body).unwrap();
    assert_eq!(updated.record_no, record.record_no);
    assert_eq!(updated.data["title"], "B");

    // data 內的 record_no 不是欄位 → 422
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri(),
        &token,
        Some(json!({ "data": { "record_no": "HACK-999" } })),
    )
    .await;
    let err = assert_error(status, &body, ErrorCode::Validation);
    assert_eq!(err.details.unwrap()["fields"][0]["field_key"], "record_no");

    // record_no 是保留的欄位代碼
    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/entities/{ENTITY}/fields"),
        &token,
        Some(json!({ "field_key": "record_no", "label": "單號", "data_type": "text" })),
    )
    .await;
    assert_invalid_field(status, &body, "field_key");

    // 取得單筆也有單號
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("{}/{}", records_uri(), record.id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["record_no"], expected);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_sorts_and_filters_by_record_no(pool: PgPool) {
    let token = admin_token(&pool).await;
    setup(&pool, &token, json!("RO-{000}")).await;
    for _ in 0..3 {
        create(&pool, &token, json!({ "data": {} })).await;
    }

    assert_eq!(
        list(&pool, &token, "sort=record_no").await,
        ["RO-001", "RO-002", "RO-003"]
    );
    assert_eq!(
        list(&pool, &token, "sort=-record_no").await,
        ["RO-003", "RO-002", "RO-001"]
    );
    assert_eq!(
        list(&pool, &token, "filter.record_no.eq=RO-002").await,
        ["RO-002"]
    );
    // contains 不分大小寫
    assert_eq!(
        list(
            &pool,
            &token,
            "filter.record_no.contains=ro-00&sort=record_no"
        )
        .await,
        ["RO-001", "RO-002", "RO-003"]
    );
    assert_eq!(
        list(&pool, &token, "filter.record_no.contains=3").await,
        ["RO-003"]
    );
    assert!(
        list(&pool, &token, "filter.record_no.eq=RO-004")
            .await
            .is_empty()
    );

    // 單號不支援其他運算子
    let (status, body) = call(
        &pool,
        Method::GET,
        &format!("{}?filter.record_no.gte=RO-001", records_uri()),
        &token,
        None,
    )
    .await;
    assert_invalid_field(status, &body, "filter.record_no.gte");
}
