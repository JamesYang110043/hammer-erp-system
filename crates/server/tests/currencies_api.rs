//! 幣別、匯率、稅別 API 的整合測試（#112；docs/architecture.md §6.1、§8.6、§10）。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::currency::Currency;
use hammer_shared::exchange_rate::{EffectiveRate, ExchangeRate};
use hammer_shared::pagination::Page;
use hammer_shared::tax_rate::TaxRate;
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
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0");
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
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

fn assert_invalid_field(status: StatusCode, body: &Value, field: &str) {
    let err = assert_error(status, body, ErrorCode::Validation);
    assert_eq!(err.details, Some(json!({ "field": field })), "{body}");
}

struct Tokens {
    admin_id: Id,
    admin: String,
    editor_id: Id,
    editor: String,
    viewer: String,
}

async fn tokens(pool: &PgPool) -> Tokens {
    let (admin_id, admin) = create_user_with_session(pool, &TestUser::new("admin", role::ADMIN))
        .await
        .unwrap();
    let (editor_id, editor) =
        create_user_with_session(pool, &TestUser::new("editor", role::EDITOR))
            .await
            .unwrap();
    let (_, viewer) = create_user_with_session(pool, &TestUser::new("viewer", role::VIEWER))
        .await
        .unwrap();
    Tokens {
        admin_id,
        admin: admin.into_string(),
        editor_id,
        editor: editor.into_string(),
        viewer: viewer.into_string(),
    }
}

async fn create_rate(
    pool: &PgPool,
    token: &str,
    currency_code: &str,
    rate_date: &str,
    rate: &str,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        "/api/v1/exchange-rates",
        Some(token),
        Some(json!({ "currency_code": currency_code, "rate_date": rate_date, "rate": rate })),
    )
    .await
}

async fn create_rate_ok(
    pool: &PgPool,
    token: &str,
    currency_code: &str,
    rate_date: &str,
    rate: &str,
) -> ExchangeRate {
    let (status, body) = create_rate(pool, token, currency_code, rate_date, rate).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn patch_rate(pool: &PgPool, token: &str, id: Id, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/exchange-rates/{id}"),
        Some(token),
        Some(body),
    )
    .await
}

async fn delete_rate(pool: &PgPool, token: &str, id: Id, version: i32) -> (StatusCode, Value) {
    call(
        pool,
        Method::DELETE,
        &format!("/api/v1/exchange-rates/{id}?version={version}"),
        Some(token),
        None,
    )
    .await
}

async fn effective(pool: &PgPool, token: &str, query: &str) -> (StatusCode, Value) {
    call(
        pool,
        Method::GET,
        &format!("/api/v1/exchange-rates/effective?{query}"),
        Some(token),
        None,
    )
    .await
}

async fn effective_ok(pool: &PgPool, token: &str, currency: &str, date: &str) -> EffectiveRate {
    let (status, body) = effective(
        pool,
        token,
        &format!("currency_code={currency}&date={date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn list_rates(pool: &PgPool, token: &str, query: &str) -> (StatusCode, Value) {
    call(
        pool,
        Method::GET,
        &format!("/api/v1/exchange-rates?{query}"),
        Some(token),
        None,
    )
    .await
}

async fn list_rates_ok(pool: &PgPool, token: &str, query: &str) -> Page<ExchangeRate> {
    let (status, body) = list_rates(pool, token, query).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn create_currency(pool: &PgPool, token: &str, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        "/api/v1/currencies",
        Some(token),
        Some(body),
    )
    .await
}

async fn patch_currency(
    pool: &PgPool,
    token: &str,
    code: &str,
    body: Value,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &format!("/api/v1/currencies/{code}"),
        Some(token),
        Some(body),
    )
    .await
}

async fn list_currencies(pool: &PgPool, token: &str, query: &str) -> Vec<Currency> {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/currencies{query}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn currency(pool: &PgPool, token: &str, code: &str) -> Currency {
    list_currencies(pool, token, "?include_inactive=true")
        .await
        .into_iter()
        .find(|c| c.code == code)
        .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    user_id: Option<Id>,
    action: String,
    entity_key: Option<String>,
    before_data: Option<Value>,
    after_data: Option<Value>,
}

async fn audit_logs(pool: &PgPool, record_id: Id) -> Vec<AuditRow> {
    sqlx::query_as(
        "SELECT user_id, action, entity_key, before_data, after_data
         FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// 權限
// ---------------------------------------------------------------------------

/// 驗收：可編輯者可以新增、修改匯率；唯讀者不行（伺服器端檢查）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn editor_can_maintain_rates_viewer_cannot(pool: PgPool) {
    let t = tokens(&pool).await;

    let rate = create_rate_ok(&pool, &t.editor, "usd", "2026-11-16", "32.15").await;
    assert_eq!(rate.currency_code, "USD");
    assert_eq!(rate.version, 1);

    let (status, body) = patch_rate(
        &pool,
        &t.editor,
        rate.id,
        json!({ "version": 1, "rate": "32.2" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated: ExchangeRate = serde_json::from_value(body).unwrap();
    assert_eq!(updated.rate.to_string(), "32.200000");
    assert_eq!(updated.version, 2);

    // 唯讀者：新增、修改、刪除都被拒絕，資料不變
    let (status, body) = create_rate(&pool, &t.viewer, "USD", "2026-11-17", "32.3").await;
    assert_error(status, &body, ErrorCode::Forbidden);
    let (status, body) = patch_rate(
        &pool,
        &t.viewer,
        rate.id,
        json!({ "version": 2, "rate": "1" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::Forbidden);
    let (status, body) = delete_rate(&pool, &t.viewer, rate.id, 2).await;
    assert_error(status, &body, ErrorCode::Forbidden);

    // 唯讀者可以查看
    let page = list_rates_ok(&pool, &t.viewer, "").await;
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0], updated);
    let found = effective_ok(&pool, &t.viewer, "USD", "2026-11-20").await;
    assert_eq!(found.rate, Some(updated.rate));

    // 可編輯者可以刪除
    let (status, body) = delete_rate(&pool, &t.editor, rate.id, 2).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(list_rates_ok(&pool, &t.viewer, "").await.total, 0);
}

/// 驗收：可編輯者不能新增、停用幣別；管理者可以。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn only_admin_manages_currencies(pool: PgPool) {
    let t = tokens(&pool).await;
    let gbp = json!({ "code": "gbp", "name": "英鎊", "symbol": "£", "decimal_places": 2 });

    for token in [&t.editor, &t.viewer] {
        let (status, body) = create_currency(&pool, token, gbp.clone()).await;
        assert_error(status, &body, ErrorCode::Forbidden);
        let (status, body) = patch_currency(
            &pool,
            token,
            "USD",
            json!({ "version": 1, "is_active": false }),
        )
        .await;
        assert_error(status, &body, ErrorCode::Forbidden);
    }
    assert!(currency(&pool, &t.viewer, "USD").await.is_active);

    let (status, body) = create_currency(&pool, &t.admin, gbp).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let created: Currency = serde_json::from_value(body).unwrap();
    assert_eq!(created.code, "GBP");
    assert_eq!(created.symbol.as_deref(), Some("£"));
    assert!(created.is_active && !created.is_base);

    let (status, body) = patch_currency(
        &pool,
        &t.admin,
        "usd",
        json!({ "version": 1, "is_active": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!serde_json::from_value::<Currency>(body).unwrap().is_active);
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn endpoints_require_login(pool: PgPool) {
    for uri in [
        "/api/v1/currencies",
        "/api/v1/exchange-rates",
        "/api/v1/exchange-rates/effective?currency_code=USD&date=2026-11-17",
        "/api/v1/tax-rates",
    ] {
        let (status, body) = call(&pool, Method::GET, uri, None, None).await;
        assert_error(status, &body, ErrorCode::Unauthorized);
    }
}

// ---------------------------------------------------------------------------
// 幣別
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn currency_list_and_rules(pool: PgPool) {
    let t = tokens(&pool).await;

    // 預設資料：本位幣優先、再依代碼排序
    let codes: Vec<_> = list_currencies(&pool, &t.viewer, "")
        .await
        .into_iter()
        .map(|c| c.code)
        .collect();
    assert_eq!(codes, ["TWD", "CNY", "EUR", "HKD", "JPY", "USD"]);

    // 代碼格式、重複（含已停用）、名稱、小數位數
    for (body, field) in [
        (
            json!({ "code": "US", "name": "x", "decimal_places": 2 }),
            "code",
        ),
        (
            json!({ "code": "U1D", "name": "x", "decimal_places": 2 }),
            "code",
        ),
        (
            json!({ "code": "usd", "name": "x", "decimal_places": 2 }),
            "code",
        ),
        (
            json!({ "code": "GBP", "name": " ", "decimal_places": 2 }),
            "name",
        ),
        (
            json!({ "code": "GBP", "name": "英鎊", "decimal_places": 5 }),
            "decimal_places",
        ),
    ] {
        let (status, res) = create_currency(&pool, &t.admin, body).await;
        assert_invalid_field(status, &res, field);
    }

    // TWD 不可停用；仍可改名稱
    let twd = currency(&pool, &t.admin, "TWD").await;
    let (status, body) = patch_currency(
        &pool,
        &t.admin,
        "TWD",
        json!({ "version": twd.version, "is_active": false }),
    )
    .await;
    assert_invalid_field(status, &body, "is_active");
    let (status, body) = patch_currency(
        &pool,
        &t.admin,
        "TWD",
        json!({ "version": twd.version, "name": "臺幣", "symbol": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let twd: Currency = serde_json::from_value(body).unwrap();
    assert_eq!(
        (twd.name.as_str(), twd.symbol, twd.version),
        ("臺幣", None, 2)
    );
    assert!(twd.is_base && twd.is_active);

    // 停用後預設列表不顯示，include_inactive 才顯示；代碼仍不可重複使用
    let jpy = currency(&pool, &t.admin, "JPY").await;
    let (status, body) = patch_currency(
        &pool,
        &t.admin,
        "JPY",
        json!({ "version": jpy.version, "is_active": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !list_currencies(&pool, &t.viewer, "")
            .await
            .iter()
            .any(|c| c.code == "JPY")
    );
    assert!(!currency(&pool, &t.viewer, "JPY").await.is_active);
    let (status, body) = create_currency(
        &pool,
        &t.admin,
        json!({ "code": "JPY", "name": "日圓", "decimal_places": 0 }),
    )
    .await;
    assert_invalid_field(status, &body, "code");

    // 不存在的幣別
    let (status, body) =
        patch_currency(&pool, &t.admin, "XYZ", json!({ "version": 1, "name": "x" })).await;
    assert_error(status, &body, ErrorCode::NotFound);
}

/// 驗收：幣別的操作紀錄與樂觀鎖定。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn currency_audit_and_optimistic_lock(pool: PgPool) {
    let t = tokens(&pool).await;
    let (status, body) = create_currency(
        &pool,
        &t.admin,
        json!({ "code": "GBP", "name": "英鎊", "decimal_places": 2 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let gbp: Currency = serde_json::from_value(body).unwrap();

    let (status, body) = patch_currency(
        &pool,
        &t.admin,
        "GBP",
        json!({ "version": 1, "symbol": "£" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 版本不符 → 409，資料不變
    let (status, body) =
        patch_currency(&pool, &t.admin, "GBP", json!({ "version": 1, "name": "x" })).await;
    assert_error(status, &body, ErrorCode::Conflict);
    let current = currency(&pool, &t.admin, "GBP").await;
    assert_eq!((current.name.as_str(), current.version), ("英鎊", 2));

    // 沒有變動時不寫入
    let (status, body) = patch_currency(
        &pool,
        &t.admin,
        "GBP",
        json!({ "version": 2, "symbol": "£" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(serde_json::from_value::<Currency>(body).unwrap().version, 2);

    let logs = audit_logs(&pool, gbp.id).await;
    assert_eq!(logs.len(), 2);
    for log in &logs {
        assert_eq!(log.user_id, Some(t.admin_id));
        assert_eq!(log.entity_key.as_deref(), Some("currencies"));
    }
    assert_eq!(logs[0].action, audit::CREATE);
    assert_eq!(logs[0].before_data, None);
    assert_eq!(
        logs[0].after_data,
        Some(json!({
            "code": "GBP", "name": "英鎊", "symbol": null, "decimal_places": 2,
            "is_active": true, "is_base": false,
        }))
    );
    assert_eq!(logs[1].action, audit::UPDATE);
    assert_eq!(logs[1].before_data, Some(json!({ "symbol": null })));
    assert_eq!(logs[1].after_data, Some(json!({ "symbol": "£" })));
}

// ---------------------------------------------------------------------------
// 匯率
// ---------------------------------------------------------------------------

/// 驗收：匯率使用 `NUMERIC(18,6)` 與 `rust_decimal`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn rate_is_numeric_18_6(pool: PgPool) {
    let t = tokens(&pool).await;
    let rate = create_rate_ok(&pool, &t.editor, "USD", "2026-11-16", "32.123456").await;
    // JSON 中是字串，固定 6 位小數
    let (_, body) = list_rates(&pool, &t.viewer, "").await;
    assert_eq!(body["items"][0]["rate"], json!("32.123456"));
    assert_eq!(rate.rate.to_string(), "32.123456");
    let small = create_rate_ok(&pool, &t.editor, "JPY", "2026-11-16", "0.2").await;
    assert_eq!(small.rate.to_string(), "0.200000");

    let column: (String,) = sqlx::query_as(
        "SELECT format_type(atttypid, atttypmod) FROM pg_attribute
         WHERE attrelid = 'exchange_rates'::regclass AND attname = 'rate'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(column.0, "numeric(18,6)");

    // 超過 6 位小數、不大於 0、超過範圍、JSON number → 拒絕
    for value in ["32.1234567", "0", "-1", "1000000000000"] {
        let (status, body) = create_rate(&pool, &t.editor, "EUR", "2026-11-16", value).await;
        assert_invalid_field(status, &body, "rate");
    }
    let (status, _) = call(
        &pool,
        Method::POST,
        "/api/v1/exchange-rates",
        Some(&t.editor),
        Some(json!({ "currency_code": "EUR", "rate_date": "2026-11-16", "rate": 35.1 })),
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    let (status, body) = patch_rate(
        &pool,
        &t.editor,
        rate.id,
        json!({ "version": 1, "rate": "0.0000001" }),
    )
    .await;
    assert_invalid_field(status, &body, "rate");
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn rate_currency_and_date_rules(pool: PgPool) {
    let t = tokens(&pool).await;

    // 本位幣、不存在的幣別
    let (status, body) = create_rate(&pool, &t.editor, "TWD", "2026-11-16", "1").await;
    assert_invalid_field(status, &body, "currency_code");
    let (status, body) = create_rate(&pool, &t.editor, "XYZ", "2026-11-16", "1").await;
    assert_invalid_field(status, &body, "currency_code");

    // 同一幣別同一天只能有一筆；不同幣別、不同日期、未來的日期可以
    let usd = create_rate_ok(&pool, &t.editor, "USD", "2026-11-16", "32").await;
    let (status, body) = create_rate(&pool, &t.editor, "usd", "2026-11-16", "33").await;
    assert_invalid_field(status, &body, "rate_date");
    create_rate_ok(&pool, &t.editor, "EUR", "2026-11-16", "35").await;
    create_rate_ok(&pool, &t.editor, "USD", "2030-01-01", "30").await;

    // 刪除後同一天可以重新輸入
    let (status, body) = delete_rate(&pool, &t.editor, usd.id, 1).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    create_rate_ok(&pool, &t.editor, "USD", "2026-11-16", "33").await;
    // 已刪除的匯率不能修改、刪除
    let (status, body) = patch_rate(
        &pool,
        &t.editor,
        usd.id,
        json!({ "version": 2, "rate": "1" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::NotFound);
    let (status, body) = delete_rate(&pool, &t.editor, usd.id, 2).await;
    assert_error(status, &body, ErrorCode::NotFound);

    // 停用的幣別：不可新增或修改匯率；既有匯率仍可查看
    let eur_rate = list_rates_ok(&pool, &t.viewer, "currency_code=EUR")
        .await
        .items[0]
        .clone();
    let eur = currency(&pool, &t.admin, "EUR").await;
    let (status, body) = patch_currency(
        &pool,
        &t.admin,
        "EUR",
        json!({ "version": eur.version, "is_active": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = create_rate(&pool, &t.editor, "EUR", "2026-11-17", "36").await;
    assert_invalid_field(status, &body, "currency_code");
    let (status, body) = patch_rate(
        &pool,
        &t.editor,
        eur_rate.id,
        json!({ "version": 1, "rate": "36" }),
    )
    .await;
    assert_invalid_field(status, &body, "currency_code");
    assert_eq!(
        list_rates_ok(&pool, &t.viewer, "currency_code=EUR")
            .await
            .total,
        1
    );
    let found = effective_ok(&pool, &t.viewer, "EUR", "2026-11-20").await;
    assert_eq!(found.rate, Some(eur_rate.rate));
}

/// 驗收：最近匯率查詢在當天有、只有之前、完全沒有三種情況都正確。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn effective_rate_cases(pool: PgPool) {
    let t = tokens(&pool).await;
    create_rate_ok(&pool, &t.editor, "USD", "2026-11-10", "31.5").await;
    create_rate_ok(&pool, &t.editor, "USD", "2026-11-15", "32.15").await;
    create_rate_ok(&pool, &t.editor, "USD", "2026-11-20", "33").await;
    create_rate_ok(&pool, &t.editor, "EUR", "2026-11-17", "35").await;

    // 當天有
    let (status, body) = effective(&pool, &t.viewer, "currency_code=USD&date=2026-11-15").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({
            "currency_code": "USD",
            "date": "2026-11-15",
            "rate": "32.150000",
            "rate_date": "2026-11-15",
        })
    );

    // 只有之前：取之前最近的一筆，不使用之後的匯率
    let (_, body) = effective(&pool, &t.viewer, "currency_code=usd&date=2026-11-17").await;
    assert_eq!(
        body,
        json!({
            "currency_code": "USD",
            "date": "2026-11-17",
            "rate": "32.150000",
            "rate_date": "2026-11-15",
        })
    );

    // 完全沒有：rate、rate_date 為 null，不是錯誤
    let (status, body) = effective(&pool, &t.viewer, "currency_code=USD&date=2026-11-09").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "currency_code": "USD", "date": "2026-11-09", "rate": null, "rate_date": null })
    );
    let none = effective_ok(&pool, &t.viewer, "HKD", "2026-11-17").await;
    assert_eq!((none.rate, none.rate_date), (None, None));

    // 刪除的匯率不採用
    let page = list_rates_ok(
        &pool,
        &t.viewer,
        "currency_code=USD&date_from=2026-11-15&date_to=2026-11-15",
    )
    .await;
    let (status, _) = delete_rate(&pool, &t.editor, page.items[0].id, 1).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let found = effective_ok(&pool, &t.viewer, "USD", "2026-11-17").await;
    assert_eq!(found.rate.unwrap().to_string(), "31.500000");

    // 本位幣固定 1
    let (_, body) = effective(&pool, &t.viewer, "currency_code=TWD&date=2026-11-17").await;
    assert_eq!(
        body,
        json!({ "currency_code": "TWD", "date": "2026-11-17", "rate": "1.000000", "rate_date": null })
    );

    // 幣別不存在 → 422；參數格式錯誤或缺少 → 400
    let (status, body) = effective(&pool, &t.viewer, "currency_code=XYZ&date=2026-11-17").await;
    assert_invalid_field(status, &body, "currency_code");
    for query in [
        "currency_code=USD&date=2026/11/17",
        "currency_code=USD",
        "date=2026-11-17",
    ] {
        let (status, body) = effective(&pool, &t.viewer, query).await;
        assert_error(status, &body, ErrorCode::BadRequest);
    }
}

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn rate_list_filters_and_pagination(pool: PgPool) {
    let t = tokens(&pool).await;
    create_rate_ok(&pool, &t.editor, "USD", "2026-11-01", "31").await;
    create_rate_ok(&pool, &t.editor, "USD", "2026-11-02", "32").await;
    create_rate_ok(&pool, &t.editor, "EUR", "2026-11-02", "35").await;
    create_rate_ok(&pool, &t.editor, "JPY", "2026-11-03", "0.21").await;

    // 依 rate_date DESC, currency_code 排序
    let page = list_rates_ok(&pool, &t.viewer, "").await;
    let keys: Vec<_> = page
        .items
        .iter()
        .map(|r| format!("{} {}", r.rate_date, r.currency_code))
        .collect();
    assert_eq!(
        keys,
        [
            "2026-11-03 JPY",
            "2026-11-02 EUR",
            "2026-11-02 USD",
            "2026-11-01 USD"
        ]
    );
    assert_eq!((page.total, page.page, page.page_size), (4, 1, 50));

    let page = list_rates_ok(&pool, &t.viewer, "currency_code=usd").await;
    assert_eq!(page.total, 2);
    // 日期區間含開始、含結束
    let page = list_rates_ok(&pool, &t.viewer, "date_from=2026-11-02&date_to=2026-11-02").await;
    assert_eq!(page.total, 2);
    let page = list_rates_ok(&pool, &t.viewer, "date_from=2026-11-02&page=2&page_size=2").await;
    assert_eq!((page.total, page.items.len()), (3, 1));
    assert_eq!(page.items[0].currency_code, "USD");
    // 空字串的幣別視為未指定
    assert_eq!(
        list_rates_ok(&pool, &t.viewer, "currency_code=")
            .await
            .total,
        4
    );

    let (status, body) =
        list_rates(&pool, &t.viewer, "date_from=2026-11-03&date_to=2026-11-02").await;
    assert_invalid_field(status, &body, "date_from");
    let (status, body) = list_rates(&pool, &t.viewer, "page=0").await;
    assert_invalid_field(status, &body, "page");
    let (status, body) = list_rates(&pool, &t.viewer, "date_from=yesterday").await;
    assert_error(status, &body, ErrorCode::BadRequest);
}

/// 驗收：匯率的操作紀錄與樂觀鎖定。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn rate_audit_and_optimistic_lock(pool: PgPool) {
    let t = tokens(&pool).await;
    let rate = create_rate_ok(&pool, &t.editor, "USD", "2026-11-16", "32.15").await;

    let (status, body) = patch_rate(
        &pool,
        &t.editor,
        rate.id,
        json!({ "version": 1, "rate": "32.2" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 版本不符 → 409，資料不變
    let (status, body) = patch_rate(
        &pool,
        &t.editor,
        rate.id,
        json!({ "version": 1, "rate": "40" }),
    )
    .await;
    assert_error(status, &body, ErrorCode::Conflict);
    let (status, body) = delete_rate(&pool, &t.editor, rate.id, 1).await;
    assert_error(status, &body, ErrorCode::Conflict);
    let current = list_rates_ok(&pool, &t.viewer, "").await.items[0].clone();
    assert_eq!(
        (current.rate.to_string().as_str(), current.version),
        ("32.200000", 2)
    );

    // 匯率相同（小數位數不同）時不寫入
    let (status, body) = patch_rate(
        &pool,
        &t.editor,
        rate.id,
        json!({ "version": 2, "rate": "32.20" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_value::<ExchangeRate>(body)
            .unwrap()
            .version,
        2
    );

    let (status, _) = delete_rate(&pool, &t.editor, rate.id, 2).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let logs = audit_logs(&pool, rate.id).await;
    let actions: Vec<_> = logs.iter().map(|l| l.action.as_str()).collect();
    assert_eq!(actions, [audit::CREATE, audit::UPDATE, audit::DELETE]);
    for log in &logs {
        assert_eq!(log.user_id, Some(t.editor_id));
        assert_eq!(log.entity_key.as_deref(), Some("exchange_rates"));
    }
    let full = json!({ "currency_code": "USD", "rate_date": "2026-11-16", "rate": "32.150000" });
    assert_eq!(logs[0].after_data, Some(full));
    assert_eq!(logs[1].before_data, Some(json!({ "rate": "32.150000" })));
    assert_eq!(logs[1].after_data, Some(json!({ "rate": "32.200000" })));
    assert_eq!(
        logs[2].before_data,
        Some(json!({ "currency_code": "USD", "rate_date": "2026-11-16", "rate": "32.200000" }))
    );
    assert_eq!(logs[2].after_data, None);
}

// ---------------------------------------------------------------------------
// 稅別
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn tax_rates_are_listed(pool: PgPool) {
    let t = tokens(&pool).await;
    let (status, body) = call(
        &pool,
        Method::GET,
        "/api/v1/tax-rates",
        Some(&t.viewer),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let taxes: Vec<TaxRate> = serde_json::from_value(body.clone()).unwrap();
    let codes: Vec<_> = taxes.iter().map(|t| t.code.as_str()).collect();
    assert_eq!(codes, ["taxable", "exempt", "zero_rated"]);
    assert_eq!(
        body[0],
        json!({ "code": "taxable", "name": "應稅", "rate": "0.0500" })
    );

    // 唯讀，沒有寫入端點
    let (status, body) = call(
        &pool,
        Method::POST,
        "/api/v1/tax-rates",
        Some(&t.admin),
        Some(json!({ "code": "x" })),
    )
    .await;
    assert_error(status, &body, ErrorCode::MethodNotAllowed);
}
