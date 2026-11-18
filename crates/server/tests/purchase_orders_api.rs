//! 採購單的整合測試（#139，docs/architecture.md §8.8）：存檔時的幣別、匯率、稅別預設值與金額計算；
//! 確認、手動結案、作廢與狀態檢查；廠商、商品被採購單據引用時的限制。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::record::Record;
use hammer_shared::{Id, role};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

fn state(pool: &PgPool) -> AppState {
    let config = Config::from_sources(None, |name| match name {
        "DATABASE_URL" => Some("postgres://unused".to_owned()),
        _ => None,
    })
    .unwrap();
    AppState::new(pool.clone(), config)
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
    let res = router(state(pool))
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

async fn user(pool: &PgPool, name: &str, role: &str) -> String {
    let (_, token) = create_user_with_session(pool, &TestUser::new(name, role))
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

async fn create(pool: &PgPool, token: &str, entity: &str, body: Value) -> Record {
    let (status, body) = call(pool, Method::POST, &records_uri(entity), token, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn patch(pool: &PgPool, token: &str, id: Id, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::PATCH,
        &record_uri("purchase_order", id),
        token,
        Some(body),
    )
    .await
}

async fn action(
    pool: &PgPool,
    token: &str,
    entity: &str,
    id: Id,
    name: &str,
    version: i32,
) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &format!("{}/{name}", record_uri(entity, id)),
        token,
        Some(json!({ "version": version })),
    )
    .await
}

struct Master {
    usd_supplier: Id,
    twd_supplier: Id,
    unit: Id,
    goods: Id,
    other: Id,
}

async fn master_data(pool: &PgPool, token: &str) -> Master {
    let supplier = |code: &str, currency: &str| {
        json!({ "data": { "code": code, "name": format!("廠商 {code}"),
                          "default_currency_code": currency } })
    };
    let usd_supplier = create(pool, token, "supplier", supplier("S001", "USD"))
        .await
        .id;
    let twd_supplier = create(pool, token, "supplier", supplier("S002", "TWD"))
        .await
        .id;
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    let unit2 = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "box", "name": "箱" } }),
    )
    .await
    .id;
    let product = |code: &str| json!({ "data": { "code": code, "name": format!("商品 {code}"), "unit_id": unit } });
    let goods = create(pool, token, "product", product("A001")).await.id;
    let other = create(pool, token, "product", product("A002")).await.id;
    let _ = unit2;
    Master {
        usd_supplier,
        twd_supplier,
        unit,
        goods,
        other,
    }
}

async fn exchange_rate(pool: &PgPool, token: &str, code: &str, date: &str, rate: &str) {
    let (status, body) = call(
        pool,
        Method::POST,
        "/api/v1/exchange-rates",
        token,
        Some(json!({ "currency_code": code, "rate_date": date, "rate": rate })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

fn lines(record: &Record) -> &[Record] {
    &record.lines.as_ref().unwrap()["purchase_order_line"]
}

fn line(product: Id, qty: &str, price: &str) -> Value {
    json!({ "data": { "product_id": product, "qty": qty, "unit_price": price } })
}

async fn audit_actions(pool: &PgPool, record_id: Id) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT action FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(record_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// 新增時沒有帶幣別、稅別、匯率：帶入廠商的預設幣別、依幣別的稅別、單據日期的適用匯率。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn create_fills_currency_tax_and_rate(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    exchange_rate(&pool, &token, "USD", "2026-11-10", "31.5").await;
    exchange_rate(&pool, &token, "USD", "2026-11-15", "32.15").await;
    // 單據日期之後的匯率不使用
    exchange_rate(&pool, &token, "USD", "2026-11-20", "40").await;

    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-17" },
            "lines": { "purchase_order_line": [
                line(m.goods, "10", "12.345"), line(m.other, "3", "7.5"),
            ] },
        }),
    )
    .await;
    assert_eq!(po.data["currency_code"], "USD");
    assert_eq!(po.data["tax_type"], "zero_rated");
    assert_eq!(po.data["exchange_rate"], "32.150000");
    // §8.8 範例二
    assert_eq!(lines(&po)[0].data["amount"], "123.4500");
    assert_eq!(lines(&po)[1].data["amount"], "22.5000");
    assert_eq!(po.data["subtotal"], "145.9500");
    assert_eq!(po.data["tax_amount"], "0.0000");
    assert_eq!(po.data["total"], "145.9500");
    assert_eq!(po.data["total_twd"], "4692.29");

    // 找不到匯率：留空，台幣總計為空
    let early = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-01" },
            "lines": { "purchase_order_line": [line(m.goods, "1", "10")] },
        }),
    )
    .await;
    assert!(!early.data.contains_key("exchange_rate"));
    assert!(!early.data.contains_key("total_twd"));
    assert_eq!(early.data["total"], "10.0000");

    // 台幣廠商：應稅、匯率固定 1（request 帶了其他值也覆蓋）；§8.8 範例一
    let twd = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.twd_supplier, "order_date": "2026-11-17",
                      "exchange_rate": "30" },
            "lines": { "purchase_order_line": [
                line(m.goods, "3", "33.3333"), line(m.other, "2", "150.5"),
            ] },
        }),
    )
    .await;
    assert_eq!(twd.data["currency_code"], "TWD");
    assert_eq!(twd.data["tax_type"], "taxable");
    assert_eq!(twd.data["exchange_rate"], "1.000000");
    assert_eq!(lines(&twd)[0].data["amount"], "100.0000");
    assert_eq!(lines(&twd)[1].data["amount"], "301.0000");
    assert_eq!(twd.data["subtotal"], "401.0000");
    assert_eq!(twd.data["tax_amount"], "20.0000");
    assert_eq!(twd.data["total"], "421.0000");
    assert_eq!(twd.data["total_twd"], "421.00");

    // 使用者指定幣別、稅別、匯率：外幣應稅
    let taxed = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.twd_supplier, "order_date": "2026-11-17",
                      "currency_code": "USD", "tax_type": "taxable", "exchange_rate": "30.5" },
            "lines": { "purchase_order_line": [line(m.goods, "1", "10.01")] },
        }),
    )
    .await;
    assert_eq!(taxed.data["exchange_rate"], "30.500000");
    assert_eq!(taxed.data["tax_amount"], "0.5000");
    assert_eq!(taxed.data["total"], "10.5100");
    assert_eq!(taxed.data["total_twd"], "320.56");
}

/// 存檔時的檢查：幣別、匯率、數量、單價。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn save_validates_currency_rate_and_lines(pool: PgPool) {
    let admin = user(&pool, "admin", role::ADMIN).await;
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("purchase_order"),
        &token,
        Some(json!({
            "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-17",
                      "exchange_rate": "0" },
            "lines": { "purchase_order_line": [
                line(m.goods, "0", "1"), line(m.goods, "1", "-1"),
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "exchange_rate", "message": "匯率必須大於 0" }])
    );
    let lines_err = &body["details"]["lines"];
    assert_eq!(lines_err[0]["index"], 0);
    assert_eq!(lines_err[0]["field_key"], "qty");
    assert_eq!(lines_err[0]["message"], "數量必須大於 0");
    assert_eq!(lines_err[1]["index"], 1);
    assert_eq!(lines_err[1]["field_key"], "unit_price");

    // 匯率超過 6 位小數：不自動四捨五入
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("purchase_order"),
        &token,
        Some(
            json!({ "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-17",
                               "exchange_rate": "32.1234567" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["fields"][0]["field_key"], "exchange_rate");

    // 停用的幣別不可用於新的單據；既有草稿保留原值仍可修改其他欄位
    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({ "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-17",
                          "currency_code": "EUR", "exchange_rate": "35" } }),
    )
    .await;
    let (_, eur) = call(&pool, Method::GET, "/api/v1/currencies", &admin, None).await;
    let version = eur
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["code"] == "EUR")
        .unwrap()["version"]
        .clone();
    let (status, body) = call(
        &pool,
        Method::PATCH,
        "/api/v1/currencies/EUR",
        &admin,
        Some(json!({ "version": version, "is_active": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("purchase_order"),
        &token,
        Some(
            json!({ "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-17",
                               "currency_code": "EUR" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "currency_code", "message": "幣別不存在或已停用" }])
    );
    let (status, body) = patch(
        &pool,
        &token,
        po.id,
        json!({ "version": po.version, "data": { "notes": "保留歐元" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["currency_code"], "EUR");
}

/// 修改：幣別變動時重新帶入匯率並以新的小數位數重算明細；只改日期不更新匯率；改明細重算合計。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn update_recalculates_amounts(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    exchange_rate(&pool, &token, "USD", "2026-11-01", "32").await;
    exchange_rate(&pool, &token, "USD", "2026-11-15", "33").await;
    exchange_rate(&pool, &token, "JPY", "2026-11-01", "0.21").await;

    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-10" },
            "lines": { "purchase_order_line": [line(m.goods, "3", "10.555")] },
        }),
    )
    .await;
    assert_eq!(po.data["exchange_rate"], "32.000000");
    assert_eq!(lines(&po)[0].data["amount"], "31.6700");
    assert_eq!(po.data["total_twd"], "1013.44");

    // 只改日期：匯率不自動更新
    let (status, body) = patch(
        &pool,
        &token,
        po.id,
        json!({ "version": po.version, "data": { "order_date": "2026-11-16" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["exchange_rate"], "32.000000");
    let version = body["version"].as_i64().unwrap();

    // 改幣別（日圓，小數 0 位）、沒有帶匯率：帶入適用匯率，明細金額依新的小數位數重算
    let (status, body) = patch(
        &pool,
        &token,
        po.id,
        json!({ "version": version, "data": { "currency_code": "JPY" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["exchange_rate"], "0.210000");
    let first = &body["lines"]["purchase_order_line"][0];
    assert_eq!(first["data"]["amount"], "32.0000");
    assert_eq!(body["data"]["total"], "32.0000");
    assert_eq!(body["data"]["total_twd"], "6.72");
    let version = body["version"].as_i64().unwrap();
    let line_id = first["id"].clone();

    // 改明細：合計重算；使用者輸入匯率
    let (status, body) = patch(
        &pool,
        &token,
        po.id,
        json!({
            "version": version,
            "data": { "exchange_rate": "0.2" },
            "lines": { "purchase_order_line": [
                { "id": line_id, "data": { "qty": "5" } },
                line(m.other, "1", "100"),
            ] },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["lines"]["purchase_order_line"][0]["data"]["amount"],
        "53.0000"
    );
    assert_eq!(body["data"]["subtotal"], "153.0000");
    assert_eq!(body["data"]["total_twd"], "30.60");
    let version = body["version"].as_i64().unwrap();

    // 改為人民幣、沒有帶匯率：找不到適用匯率時清空
    let (status, body) = patch(
        &pool,
        &token,
        po.id,
        json!({ "version": version, "data": { "currency_code": "CNY" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"].get("exchange_rate").is_none(), "{body}");
    assert!(body["data"].get("total_twd").is_none(), "{body}");

    // 沒有任何變動：version 不變
    let version = body["version"].as_i64().unwrap();
    let (status, body) = patch(
        &pool,
        &token,
        po.id,
        json!({ "version": version, "data": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"].as_i64().unwrap(), version);
}

/// 確認、結案、作廢與狀態檢查。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn confirm_close_and_void(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let viewer = user(&pool, "viewer", role::VIEWER).await;
    let m = master_data(&pool, &token).await;

    // 沒有明細、沒有匯率 → 422
    let empty = create(
        &pool,
        &token,
        "purchase_order",
        json!({ "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-17" } }),
    )
    .await;
    let (status, body) = action(&pool, &token, "purchase_order", empty.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "VALIDATION");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "exchange_rate", "message": "找不到匯率，請輸入匯率" }])
    );
    assert_eq!(body["details"]["lines"][0]["message"], "至少要有一筆明細");
    assert_eq!(body["details"]["lines"][0]["index"], Value::Null);

    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.usd_supplier, "order_date": "2026-11-17",
                      "exchange_rate": "32" },
            "lines": { "purchase_order_line": [line(m.goods, "2", "10")] },
        }),
    )
    .await;

    // 權限、不支援的動作
    let (status, _) = action(&pool, &viewer, "purchase_order", po.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = action(&pool, &token, "purchase_order", po.id, "post", 1).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = action(&pool, &token, "purchase_order", po.id, "close", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = action(&pool, &token, "purchase_order", po.id, "confirm", 9).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // 確認
    let (status, body) = action(&pool, &token, "purchase_order", po.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "confirmed");
    assert_eq!(body["version"], 2);
    assert!(body["document"]["posted_at"].is_string());
    assert!(body["document"]["closed_at"].is_null());
    assert_eq!(body["data"]["total_twd"], "640.00");

    // 已確認：不可修改、刪除、再確認
    let (status, body) = patch(
        &pool,
        &token,
        po.id,
        json!({ "version": 2, "data": { "notes": "x" } }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "INVALID_STATUS");
    assert_eq!(body["message"], "已確認的單據不可修改");
    assert_eq!(body["details"], json!({ "status": "confirmed" }));
    let (status, _) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=2", record_uri("purchase_order", po.id)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = action(&pool, &token, "purchase_order", po.id, "confirm", 2).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "INVALID_STATUS");

    // 手動結案
    let (status, body) = action(&pool, &token, "purchase_order", po.id, "close", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "closed");
    assert!(body["document"]["closed_at"].is_string());
    assert!(body["document"]["closed_by"].is_string());
    assert!(body["document"]["posted_at"].is_string());
    // 已結案不可作廢
    let (status, body) = action(&pool, &token, "purchase_order", po.id, "void", 3).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["details"], json!({ "status": "closed" }));
    assert_eq!(
        audit_actions(&pool, po.id).await,
        ["create", "confirm", "close"]
    );

    // 作廢：只有已確認
    let po2 = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.twd_supplier, "order_date": "2026-11-17" },
            "lines": { "purchase_order_line": [line(m.goods, "1", "10")] },
        }),
    )
    .await;
    let (status, _) = action(&pool, &token, "purchase_order", po2.id, "void", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = action(&pool, &token, "purchase_order", po2.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = action(&pool, &token, "purchase_order", po2.id, "void", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "void");
    assert!(body["document"]["voided_at"].is_string());
    assert_eq!(
        audit_actions(&pool, po2.id).await,
        ["create", "confirm", "void"]
    );

    // 部分進貨：可以結案、不可作廢（進貨過帳在 #140，這裡直接設定狀態）
    let po3 = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.twd_supplier, "order_date": "2026-11-17" },
            "lines": { "purchase_order_line": [line(m.goods, "1", "10")] },
        }),
    )
    .await;
    let (status, _) = action(&pool, &token, "purchase_order", po3.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE purchase_orders SET status = 'partially_received' WHERE id = $1")
        .bind(po3.id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = action(&pool, &token, "purchase_order", po3.id, "void", 2).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["message"], "部分進貨的單據不可作廢");
    let (status, body) = action(&pool, &token, "purchase_order", po3.id, "close", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "closed");

    // 其他單據不支援確認、結案
    let adjustment = create(
        &pool,
        &token,
        "stock_adjustment",
        json!({ "data": { "adjustment_date": "2026-11-17" } }),
    )
    .await;
    for name in ["confirm", "close"] {
        let (status, _) = action(&pool, &token, "stock_adjustment", adjustment.id, name, 1).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}");
    }
}

/// 確認時廠商、商品已刪除 → 422。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn confirm_rejects_deleted_references(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.twd_supplier, "order_date": "2026-11-17" },
            "lines": { "purchase_order_line": [
                line(m.goods, "1", "10"), line(m.other, "1", "10"),
            ] },
        }),
    )
    .await;
    // 草稿引用的商品可以刪除（只限制未結案的採購單）
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("product", m.other)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    sqlx::query("UPDATE suppliers SET deleted_at = now() WHERE id = $1")
        .bind(m.twd_supplier)
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = action(&pool, &token, "purchase_order", po.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "supplier_id", "message": "廠商已刪除" }])
    );
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "purchase_order_line", "index": 1, "field_key": "product_id",
                 "message": "商品已刪除" }])
    );
    // rollback：仍是草稿
    let (_, body) = call(
        &pool,
        Method::GET,
        &record_uri("purchase_order", po.id),
        &token,
        None,
    )
    .await;
    assert_eq!(body["data"]["status"], "draft");
    assert_eq!(body["version"], 1);
}

/// 廠商有採購單據不可刪除；商品有未結案的採購單不可刪除、改為服務、修改單位。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn referenced_supplier_and_product_restrictions(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.twd_supplier, "order_date": "2026-11-17" },
            "lines": { "purchase_order_line": [line(m.goods, "1", "10")] },
        }),
    )
    .await;

    // 廠商：草稿也算
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("supplier", m.twd_supplier)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        "此廠商已有 1 張採購或進貨單據，無法刪除，請改為停用"
    );
    assert_eq!(
        body["details"],
        json!({ "fields": [], "document_count": 1 })
    );
    // 沒有單據的廠商可以刪除
    let (status, _) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("supplier", m.usd_supplier)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 商品：確認後限制
    let (status, _) = action(&pool, &token, "purchase_order", po.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("product", m.goods)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["message"], "此商品有 1 張未結案的採購單，無法刪除");
    assert_eq!(
        body["details"],
        json!({ "fields": [], "open_purchase_order_count": 1 })
    );

    let (_, units) = call(&pool, Method::GET, &records_uri("unit"), &token, None).await;
    let other_unit = units["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] != json!(m.unit))
        .unwrap()["id"]
        .clone();
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("product", m.goods),
        &token,
        Some(json!({ "version": 1,
                     "data": { "product_type": "service", "unit_id": other_unit } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([
            { "field_key": "product_type", "message": "此商品有未結案的採購單，無法改為服務" },
            { "field_key": "unit_id", "message": "此商品有未結案的採購單，無法修改單位" },
        ])
    );
    // 其他欄位可以修改
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri("product", m.goods),
        &token,
        Some(json!({ "version": 1, "data": { "name": "改名" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 結案後不再限制
    let (status, _) = action(&pool, &token, "purchase_order", po.id, "close", 2).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=2", record_uri("product", m.goods)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}
