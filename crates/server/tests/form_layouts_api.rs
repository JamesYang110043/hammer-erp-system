//! 表單版面（form_layouts）API 的整合測試（#221）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::form_layout::FormLayout;
use hammer_shared::{ApiError, ErrorCode, Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const ENTITY: &str = "repair_order";
const DETAIL: &str = "repair_part";

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
fn assert_invalid(status: StatusCode, body: &Value, field: &str) {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": field })), "{body}");
}

async fn token_for(pool: &PgPool, username: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(username, role))
        .await
        .unwrap();
    token.into_string()
}

/// 建立資料類別 [`ENTITY`]（欄位 `title`、`qty`、`note`（不顯示））與明細子表 [`DETAIL`]，
/// 回傳管理者 token。
async fn setup(pool: &PgPool) -> String {
    let admin = token_for(pool, "admin", role::ADMIN).await;
    let (status, master) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        &admin,
        Some(json!({ "key": ENTITY, "label": "維修單", "kind": "master" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{master}");
    let master_id = master["entity"]["id"].clone();

    for field in [
        json!({ "field_key": "title", "label": "標題", "data_type": "text", "sort_order": 1 }),
        json!({ "field_key": "qty", "label": "數量", "data_type": "integer", "sort_order": 2 }),
        json!({ "field_key": "note", "label": "備註", "data_type": "long_text", "sort_order": 3,
                "is_visible": false }),
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

    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/entities",
        &admin,
        Some(json!({ "key": DETAIL, "label": "零件", "kind": "detail",
                     "parent_entity_id": master_id })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    admin
}

fn uri(key: &str) -> String {
    format!("/api/v1/entities/{key}/form-layout")
}

async fn get_layout(pool: &PgPool, token: &str, key: &str) -> FormLayout {
    let (status, body) = call(pool, Method::GET, &uri(key), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn put_raw(
    pool: &PgPool,
    token: &str,
    version: Option<i32>,
    config: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::PUT,
        &uri(ENTITY),
        token,
        Some(json!({ "version": version, "config": config })),
    )
    .await
}

async fn put(pool: &PgPool, token: &str, version: Option<i32>, config: Value) -> FormLayout {
    let (status, body) = put_raw(pool, token, version, config).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn reset_raw(pool: &PgPool, token: &str, version: i32) -> (StatusCode, Value) {
    call(
        pool,
        Method::DELETE,
        &format!("{}?version={version}", uri(ENTITY)),
        token,
        None,
    )
    .await
}

/// 兩欄的基本資料區塊與明細區塊；`qty` 沒有放進版面。
fn custom_config() -> Value {
    json!({ "sections": [
        { "key": "basic", "title": "基本資料", "columns": 2, "items": [
            { "type": "block", "block": "record_no" },
            { "type": "field", "field": "title", "span": 2 },
            { "type": "spacer" },
            { "type": "field", "field": "note" },
        ]},
        { "key": "parts", "title": "零件", "columns": 1, "collapsible": true, "items": [
            { "type": "block", "block": format!("lines:{DETAIL}") },
        ]},
    ]})
}

/// 版面中各區塊的代碼與項目（`field` / `block` / `spacer`）。
fn outline(layout: &FormLayout) -> Value {
    let v = serde_json::to_value(&layout.config).unwrap();
    Value::Array(
        v["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let items: Vec<Value> = s["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| {
                        i.get("field")
                            .or_else(|| i.get("block"))
                            .cloned()
                            .unwrap_or_else(|| json!("spacer"))
                    })
                    .collect();
                json!([s["key"], items])
            })
            .collect(),
    )
}

/// 操作紀錄：`(action, record_id, before, after)`，依寫入順序。
async fn audits(pool: &PgPool) -> Vec<(String, Id, Option<Value>, Option<Value>)> {
    sqlx::query_as(
        "SELECT action, record_id, before_data, after_data FROM audit_logs
         WHERE entity_key = 'form_layout' ORDER BY occurred_at, id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn generated_layout_when_not_set(pool: PgPool) {
    let admin = setup(&pool).await;
    let layout = get_layout(&pool, &admin, ENTITY).await;
    assert!(layout.is_default);
    assert_eq!((layout.id, layout.version), (None, None));
    assert_eq!(
        outline(&layout),
        json!([[
            "main",
            ["record_no", "title", "qty", format!("lines:{DETAIL}")]
        ]])
    );

    // 系統單據：銷項發票有「選擇出貨單」，位置在明細之前
    let layout = get_layout(&pool, &admin, "sales_invoice").await;
    let items = &outline(&layout)[0][1];
    let codes: Vec<&str> = items
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let picker = codes.iter().position(|c| *c == "shipment_picker").unwrap();
    let lines = codes.iter().position(|c| c.starts_with("lines:")).unwrap();
    assert_eq!(codes[0], "record_no");
    assert!(picker < lines, "{codes:?}");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn admin_saves_layout_and_writes_are_audited(pool: PgPool) {
    let admin = setup(&pool).await;
    let created = put(&pool, &admin, None, custom_config()).await;
    assert!(!created.is_default);
    assert_eq!(created.version, Some(1));
    // 沒有放進版面的 qty 自動附加到「其他欄位」
    assert_eq!(
        outline(&created),
        json!([
            ["basic", ["record_no", "title", "spacer", "note"]],
            ["parts", [format!("lines:{DETAIL}")]],
            ["_other", ["qty"]],
        ])
    );
    assert_eq!(get_layout(&pool, &admin, ENTITY).await, created);

    // 沒有變動時不寫入
    let same = put(&pool, &admin, Some(1), custom_config()).await;
    assert_eq!(same.version, Some(1));

    // 把自動附加的區塊存回去：_other 改名為 other
    let saved = put(
        &pool,
        &admin,
        Some(1),
        serde_json::to_value(&created.config).unwrap(),
    )
    .await;
    assert_eq!(saved.version, Some(2));
    assert_eq!(outline(&saved)[2], json!(["other", ["qty"]]));

    let logs = audits(&pool).await;
    let id = created.id.unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!((logs[0].0.as_str(), logs[0].1), ("create", id));
    assert_eq!(logs[0].3.as_ref().unwrap()["entity_key"], json!(ENTITY));
    assert_eq!((logs[1].0.as_str(), logs[1].1), ("update", id));
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn viewer_can_read_but_not_modify(pool: PgPool) {
    let admin = setup(&pool).await;
    let viewer = token_for(&pool, "viewer", role::VIEWER).await;
    let editor = token_for(&pool, "editor", role::EDITOR).await;

    let saved = put(&pool, &admin, None, custom_config()).await;
    assert_eq!(get_layout(&pool, &viewer, ENTITY).await, saved);

    for token in [&viewer, &editor] {
        let (status, body) = put_raw(&pool, token, Some(1), json!({ "sections": [] })).await;
        assert_error(status, &body, ErrorCode::Forbidden);
        let (status, body) = reset_raw(&pool, token, 1).await;
        assert_error(status, &body, ErrorCode::Forbidden);
    }
    assert_eq!(get_layout(&pool, &admin, ENTITY).await, saved);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn version_mismatch_is_a_conflict(pool: PgPool) {
    let admin = setup(&pool).await;
    put(&pool, &admin, None, custom_config()).await;

    // 已有版面時不能再「第一次建立」
    let (status, body) = put_raw(&pool, &admin, None, custom_config()).await;
    assert_error(status, &body, ErrorCode::Conflict);
    // 版本不符
    let (status, body) = put_raw(
        &pool,
        &admin,
        Some(9),
        json!({ "sections": [
            { "key": "a", "columns": 1 }
        ]}),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    let (status, body) = reset_raw(&pool, &admin, 9).await;
    assert_error(status, &body, ErrorCode::Conflict);
    assert_eq!(get_layout(&pool, &admin, ENTITY).await.version, Some(1));
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_config_is_rejected(pool: PgPool) {
    let admin = setup(&pool).await;
    let cases = [
        (json!({ "sections": [] }), "config.sections"),
        (
            json!({ "sections": [{ "key": "a", "columns": 5 }] }),
            "config.sections[0].columns",
        ),
        (
            json!({ "sections": [{ "key": "a", "columns": 1,
                     "items": [{ "type": "field", "field": "missing" }] }] }),
            "config.sections[0].items[0].field",
        ),
        (
            json!({ "sections": [{ "key": "a", "columns": 1,
                     "items": [{ "type": "field", "field": "title", "span": 2 }] }] }),
            "config.sections[0].items[0].span",
        ),
        (
            json!({ "sections": [{ "key": "a", "columns": 1,
                     "items": [{ "type": "block", "block": "shipment_picker" }] }] }),
            "config.sections[0].items[0].block",
        ),
    ];
    for (config, field) in cases {
        let (status, body) = put_raw(&pool, &admin, None, config).await;
        assert_invalid(status, &body, field);
    }
    assert!(get_layout(&pool, &admin, ENTITY).await.is_default);
    assert!(audits(&pool).await.is_empty());
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn deactivated_field_is_skipped_on_read(pool: PgPool) {
    let admin = setup(&pool).await;
    put(&pool, &admin, None, custom_config()).await;

    let (status, field) = call(
        &pool,
        Method::GET,
        &format!("/api/v1/entities/{ENTITY}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{field}");
    let title = field["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["field_key"] == "title")
        .unwrap()
        .clone();
    let (status, body) = call(
        &pool,
        Method::POST,
        &format!("/api/v1/entities/{ENTITY}/fields/title/deactivate"),
        &admin,
        Some(json!({ "version": title["version"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let layout = get_layout(&pool, &admin, ENTITY).await;
    assert_eq!(
        outline(&layout),
        json!([
            ["basic", ["record_no", "spacer", "note"]],
            ["parts", [format!("lines:{DETAIL}")]],
            ["_other", ["qty"]],
        ])
    );
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn reset_returns_to_generated_layout(pool: PgPool) {
    let admin = setup(&pool).await;
    let saved = put(&pool, &admin, None, custom_config()).await;

    let (status, body) = reset_raw(&pool, &admin, 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reset: FormLayout = serde_json::from_value(body).unwrap();
    assert!(reset.is_default);
    assert_eq!(get_layout(&pool, &admin, ENTITY).await, reset);

    // 沒有版面時不能再還原
    let (status, body) = reset_raw(&pool, &admin, 2).await;
    assert_error(status, &body, ErrorCode::NotFound);

    // 還原後可以重新建立
    let again = put(&pool, &admin, None, custom_config()).await;
    assert_ne!(again.id, saved.id);
    assert_eq!(again.version, Some(1));

    let logs = audits(&pool).await;
    let actions: Vec<&str> = logs.iter().map(|l| l.0.as_str()).collect();
    assert_eq!(actions, ["create", "delete", "create"]);
    assert_eq!(logs[1].3, None);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn detail_and_unknown_entities_are_not_found(pool: PgPool) {
    let admin = setup(&pool).await;
    for key in [DETAIL, "nothing"] {
        let (status, body) = call(&pool, Method::GET, &uri(key), &admin, None).await;
        assert_error(status, &body, ErrorCode::NotFound);
    }
}

/// 客戶、廠商沒有設定版面時為分組、2 欄的預設版面（#254）；還原成預設後回到分組版面。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn customer_and_supplier_use_grouped_default_layout(pool: PgPool) {
    let admin = token_for(&pool, "admin", role::ADMIN).await;
    let customer = get_layout(&pool, &admin, "customer").await;
    assert!(customer.is_default);
    assert_eq!(
        outline(&customer),
        json!([
            [
                "basic",
                [
                    "record_no",
                    "code",
                    "name",
                    "short_name",
                    "tax_id",
                    "phone",
                    "fax",
                    "email",
                    "address",
                    "is_active"
                ]
            ],
            ["invoice", ["invoice_title", "invoice_address"]],
            ["terms", ["payment_terms_days", "credit_limit"]],
            ["other", ["notes"]],
            ["_blocks", ["lines:customer_contact"]],
        ])
    );
    let sections = &customer.config.sections;
    assert_eq!(
        sections
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        ["基本資料", "發票資訊", "交易條件", "其他", ""]
    );
    assert!(
        sections[..4]
            .iter()
            .all(|s| s.columns == 2 && !s.collapsible)
    );
    assert_eq!(
        serde_json::to_value(&sections[3].items[0]).unwrap(),
        json!({ "type": "field", "field": "notes", "span": 2 })
    );

    let supplier = get_layout(&pool, &admin, "supplier").await;
    assert!(supplier.is_default);
    assert_eq!(
        outline(&supplier),
        json!([
            [
                "basic",
                [
                    "record_no",
                    "code",
                    "name",
                    "short_name",
                    "tax_id",
                    "phone",
                    "fax",
                    "email",
                    "address",
                    "country",
                    "is_active"
                ]
            ],
            ["invoice", ["invoice_title", "invoice_address"]],
            [
                "terms",
                [
                    "payment_terms_days",
                    "credit_limit",
                    "default_currency_code",
                    "bank_info"
                ]
            ],
            ["other", ["notes"]],
            ["_blocks", ["lines:supplier_contact"]],
        ])
    );

    // 自訂版面後還原成預設：回到分組版面
    let (status, body) = call(
        &pool,
        Method::PUT,
        &uri("customer"),
        &admin,
        Some(json!({ "version": null, "config": { "sections": [
            { "key": "all", "columns": 1, "items": [{ "type": "field", "field": "name" }] }
        ] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", uri("customer")),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reset: FormLayout = serde_json::from_value(body).unwrap();
    assert!(reset.is_default);
    assert_eq!(reset.config, customer.config);
}
