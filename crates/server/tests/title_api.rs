//! 資料標題的 API 整合測試（#69）：資料類別的 `title_template` 只能引用存在的欄位，
//! 通用資料 API 的列表與單筆回應都有 `title`。
//!
//! 各型態的格式、空值、停用欄位、預設規則的細節在 `hammer_core::title` 的單元測試。
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

/// 422 VALIDATION，且 `details.field` 為 `title_template`。
fn assert_invalid_template(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let err: ApiError = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(err.code, ErrorCode::Validation, "{body}");
    assert!(!err.message.is_empty());
    assert_eq!(err.details.unwrap()["field"], "title_template", "{body}");
}

async fn admin_token(pool: &PgPool) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    token.into_string()
}

fn entity_uri() -> String {
    format!("/api/v1/entities/{ENTITY}")
}

fn records_uri() -> String {
    format!("/api/v1/entities/{ENTITY}/records")
}

/// 建立資料類別 [`ENTITY`]（單號規則 `RO-{000}`）。
async fn create_entity(pool: &PgPool, token: &str, title_template: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        "/api/v1/entities",
        token,
        Some(json!({
            "key": ENTITY, "label": "維修單", "kind": "master",
            "numbering_pattern": "RO-{000}", "title_template": title_template,
        })),
    )
    .await
}

async fn create_field(pool: &PgPool, token: &str, body: Value) {
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("{}/fields", entity_uri()),
        token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn get_entity(pool: &PgPool, token: &str) -> EntityDetail {
    let (status, body) = call(pool, Method::GET, &entity_uri(), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// 停用欄位。
async fn deactivate(pool: &PgPool, token: &str, field_key: &str) {
    let detail = get_entity(pool, token).await;
    let field = detail
        .fields
        .iter()
        .find(|f| f.field_key == field_key)
        .unwrap();
    let (status, body) = call(
        pool,
        Method::POST,
        &format!("{}/fields/{field_key}/deactivate", entity_uri()),
        token,
        Some(json!({ "version": field.version })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn set_template(pool: &PgPool, token: &str, template: Value) -> (StatusCode, Value) {
    let entity = get_entity(pool, token).await.entity;
    call(
        pool,
        Method::PATCH,
        &entity_uri(),
        token,
        Some(json!({
            "version": entity.version, "label": entity.label, "icon": entity.icon,
            "numbering_pattern": entity.numbering_pattern, "title_template": template,
            "sort_order": entity.sort_order, "is_active": entity.is_active,
        })),
    )
    .await
}

/// 建立資料類別與欄位 `name`（文字）、`status`（下拉單選）、`urgent`（勾選）。
async fn setup(pool: &PgPool, token: &str) {
    let (status, body) = create_entity(pool, token, Value::Null).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    create_field(
        pool,
        token,
        json!({ "field_key": "name", "label": "名稱", "data_type": "text" }),
    )
    .await;
    create_field(
        pool,
        token,
        json!({
            "field_key": "status", "label": "狀態", "data_type": "select",
            "options": [{ "value": "open", "label": "處理中" }, { "value": "done", "label": "已完成" }],
        }),
    )
    .await;
    create_field(
        pool,
        token,
        json!({ "field_key": "urgent", "label": "急件", "data_type": "boolean" }),
    )
    .await;
}

async fn create_record(pool: &PgPool, token: &str, data: Value) -> Record {
    let (status, body) = call(
        pool,
        Method::POST,
        &records_uri(),
        token,
        Some(json!({ "data": data })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body["title"].is_string(), "{body}");
    serde_json::from_value(body).unwrap()
}

async fn get_title(pool: &PgPool, token: &str, record: &Record) -> String {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("{}/{}", records_uri(), record.id),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["title"].as_str().unwrap().to_owned()
}

/// 列表中各筆資料的 `title`（建立順序由新到舊）。
async fn list_titles(pool: &PgPool, token: &str) -> Vec<String> {
    let (status, body) = call(pool, Method::GET, &records_uri(), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for item in body["items"].as_array().unwrap() {
        assert!(item["title"].is_string(), "{item}");
    }
    let page: Page<Record> = serde_json::from_value(body).unwrap();
    page.items.into_iter().map(|r| r.title).collect()
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn template_must_reference_existing_fields(pool: PgPool) {
    let token = admin_token(&pool).await;

    // 新的資料類別還沒有欄位，只能引用 record_no
    for template in [
        "{name}",
        "{record_no} {name}",
        "{customer.name}",
        "{record_no",
    ] {
        let (status, body) = create_entity(&pool, &token, json!(template)).await;
        assert_invalid_template(status, &body);
    }
    let (status, body) = create_entity(&pool, &token, json!(" RO {record_no} ")).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let detail: EntityDetail = serde_json::from_value(body).unwrap();
    assert_eq!(
        detail.entity.title_template.as_deref(),
        Some("RO {record_no}")
    );

    create_field(
        &pool,
        &token,
        json!({ "field_key": "name", "label": "名稱", "data_type": "text" }),
    )
    .await;

    // 修改：引用不存在的欄位、關聯欄位的欄位、語法錯誤
    for template in ["{name} {missing}", "{name.label}", "{name"] {
        let (status, body) = set_template(&pool, &token, json!(template)).await;
        assert_invalid_template(status, &body);
    }
    // 失敗的修改不會寫入
    assert_eq!(
        get_entity(&pool, &token)
            .await
            .entity
            .title_template
            .as_deref(),
        Some("RO {record_no}")
    );

    let (status, body) = set_template(&pool, &token, json!("{record_no} {name}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 停用後的欄位仍可被引用（視為空值）
    deactivate(&pool, &token, "name").await;
    let (status, body) = set_template(&pool, &token, json!("{name} / {record_no}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 空白代表清除
    let (status, body) = set_template(&pool, &token, json!("  ")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        get_entity(&pool, &token)
            .await
            .entity
            .title_template
            .is_none()
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn list_and_get_include_title(pool: PgPool) {
    let token = admin_token(&pool).await;
    setup(&pool, &token).await;

    // 沒有範本：有單號時用單號
    let first = create_record(&pool, &token, json!({ "name": "壓縮機" })).await;
    assert_eq!(first.title, "RO-001");
    assert_eq!(get_title(&pool, &token, &first).await, "RO-001");
    assert_eq!(list_titles(&pool, &token).await, ["RO-001"]);

    let (status, body) = set_template(
        &pool,
        &token,
        json!("{record_no} {name}（{status}／急件：{urgent}）"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let second = create_record(
        &pool,
        &token,
        json!({ "name": "冷氣", "status": "done", "urgent": true }),
    )
    .await;
    assert_eq!(second.title, "RO-002 冷氣（已完成／急件：是）");
    assert_eq!(
        get_title(&pool, &token, &second).await,
        "RO-002 冷氣（已完成／急件：是）"
    );
    // 範本套用到既有資料；空值為空字串
    assert_eq!(
        get_title(&pool, &token, &first).await,
        "RO-001 壓縮機（／急件：）"
    );
    assert_eq!(
        list_titles(&pool, &token).await,
        [
            "RO-002 冷氣（已完成／急件：是）",
            "RO-001 壓縮機（／急件：）"
        ]
    );

    // 修改資料後的回應也有新的標題
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &format!("{}/{}", records_uri(), first.id),
        &token,
        Some(json!({ "version": first.version, "data": { "urgent": false } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["title"], "RO-001 壓縮機（／急件：否）");

    // 引用已停用的欄位時視為空值
    deactivate(&pool, &token, "status").await;
    assert_eq!(
        get_title(&pool, &token, &second).await,
        "RO-002 冷氣（／急件：是）"
    );
}
