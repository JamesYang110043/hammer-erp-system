//! 訂單的整合測試（#147，docs/architecture.md §8.9）：存檔時的稅別預設值與金額計算（含稅、未稅、折扣）；
//! 確認、手動結案、作廢與狀態檢查；客戶、商品被銷貨單據引用時的限制。
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
        &record_uri("sales_order", id),
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
    customer: Id,
    other_customer: Id,
    unit: Id,
    goods: Id,
    other: Id,
    zero_rated: Id,
}

async fn master_data(pool: &PgPool, token: &str) -> Master {
    let customer = |code: &str| json!({ "data": { "code": code, "name": format!("客戶 {code}") } });
    let c1 = create(pool, token, "customer", customer("C001")).await.id;
    let c2 = create(pool, token, "customer", customer("C002")).await.id;
    let unit = create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "pcs", "name": "個" } }),
    )
    .await
    .id;
    create(
        pool,
        token,
        "unit",
        json!({ "data": { "code": "box", "name": "箱" } }),
    )
    .await;
    let product = |code: &str, tax: &str| {
        json!({ "data": { "code": code, "name": format!("商品 {code}"), "unit_id": unit,
                          "tax_type": tax } })
    };
    let goods = create(pool, token, "product", product("A001", "taxable"))
        .await
        .id;
    let other = create(pool, token, "product", product("A002", "taxable"))
        .await
        .id;
    let zero_rated = create(pool, token, "product", product("A003", "zero_rated"))
        .await
        .id;
    Master {
        customer: c1,
        other_customer: c2,
        unit,
        goods,
        other,
        zero_rated,
    }
}

fn lines(record: &Record) -> &[Record] {
    &record.lines.as_ref().unwrap()["sales_order_line"]
}

fn line(product: Id, qty: &str, price: &str, discount: &str) -> Value {
    json!({ "data": { "product_id": product, "qty": qty, "unit_price": price,
                      "discount_rate": discount } })
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

async fn order(pool: &PgPool, token: &str, customer: Id, lines: Vec<Value>) -> Record {
    create(
        pool,
        token,
        "sales_order",
        json!({
            "data": { "customer_id": customer, "order_date": "2026-11-18" },
            "lines": { "sales_order_line": lines },
        }),
    )
    .await
}

/// 新增時的稅別預設值與金額計算：§8.9 範例一、二、三。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn create_computes_amounts_and_default_tax(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;

    // 範例一：未稅、應稅；稅別取明細商品的稅別（都是應稅）
    let so = order(
        &pool,
        &token,
        m.customer,
        vec![
            line(m.goods, "3", "33.3333", "0"),
            line(m.other, "10", "120", "0.15"),
        ],
    )
    .await;
    assert_eq!(so.data["status"], "draft");
    assert_eq!(so.data["tax_type"], "taxable");
    assert_eq!(so.data["tax_included"], false);
    assert_eq!(lines(&so)[0].data["amount"], "100.0000");
    assert_eq!(lines(&so)[1].data["amount"], "1020.0000");
    assert_eq!(lines(&so)[1].data["shipped_qty"], "0.0000");
    assert_eq!(so.data["subtotal"], "1120.0000");
    assert_eq!(so.data["tax_amount"], "56.0000");
    assert_eq!(so.data["total"], "1176.0000");
    assert!(so.record_no.as_deref().unwrap().starts_with("SO"));

    // 範例二：含稅、應稅
    let so = create(
        &pool,
        &token,
        "sales_order",
        json!({
            "data": { "customer_id": m.customer, "order_date": "2026-11-18",
                      "tax_type": "taxable", "tax_included": true },
            "lines": { "sales_order_line": [
                line(m.goods, "2", "105", "0"), line(m.other, "5", "99", "0.1"),
            ] },
        }),
    )
    .await;
    assert_eq!(lines(&so)[1].data["amount"], "446.0000");
    assert_eq!(so.data["total"], "656.0000");
    assert_eq!(so.data["subtotal"], "625.0000");
    assert_eq!(so.data["tax_amount"], "31.0000");

    // 範例三：含稅、免稅
    let so = create(
        &pool,
        &token,
        "sales_order",
        json!({
            "data": { "customer_id": m.customer, "order_date": "2026-11-18",
                      "tax_type": "exempt", "tax_included": true },
            "lines": { "sales_order_line": [
                line(m.goods, "2", "100", "0"), line(m.other, "3", "100", "0"),
            ] },
        }),
    )
    .await;
    assert_eq!(so.data["total"], "500.0000");
    assert_eq!(so.data["subtotal"], "500.0000");
    assert_eq!(so.data["tax_amount"], "0.0000");

    // 稅別預設值：明細商品都是零稅率 → 零稅率；混合 → 應稅；沒有明細 → 應稅
    let so = order(
        &pool,
        &token,
        m.customer,
        vec![line(m.zero_rated, "1", "10", "0")],
    )
    .await;
    assert_eq!(so.data["tax_type"], "zero_rated");
    assert_eq!(so.data["tax_amount"], "0.0000");
    let so = order(
        &pool,
        &token,
        m.customer,
        vec![
            line(m.zero_rated, "1", "10", "0"),
            line(m.goods, "1", "10", "0"),
        ],
    )
    .await;
    assert_eq!(so.data["tax_type"], "taxable");
    let so = order(&pool, &token, m.customer, vec![]).await;
    assert_eq!(so.data["tax_type"], "taxable");
    assert_eq!(so.data["total"], "0.0000");

    // 系統維護的欄位：request 帶了也忽略
    let so = create(
        &pool,
        &token,
        "sales_order",
        json!({
            "data": { "customer_id": m.customer, "order_date": "2026-11-18",
                      "status": "closed", "total": "1" },
            "lines": { "sales_order_line": [
                { "data": { "product_id": m.goods, "qty": "1", "unit_price": "10",
                            "amount": "999", "shipped_qty": "5" } },
            ] },
        }),
    )
    .await;
    assert_eq!(so.data["status"], "draft");
    assert_eq!(so.data["total"], "11.0000");
    assert_eq!(lines(&so)[0].data["amount"], "10.0000");
    assert_eq!(lines(&so)[0].data["discount_rate"], "0.0000");
    assert_eq!(lines(&so)[0].data["shipped_qty"], "0.0000");
}

/// 存檔時的檢查：數量、單價、折扣率。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn save_validates_lines(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_order"),
        &token,
        Some(json!({
            "data": { "customer_id": m.customer, "order_date": "2026-11-18" },
            "lines": { "sales_order_line": [
                line(m.goods, "0", "1", "0"),
                line(m.goods, "1", "-1", "0"),
                line(m.goods, "1", "1", "1.5"),
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let errors = body["details"]["lines"].as_array().unwrap();
    let at = |i: usize| {
        errors
            .iter()
            .find(|e| e["index"] == i)
            .unwrap_or_else(|| panic!("{body}"))
    };
    assert_eq!(at(0)["field_key"], "qty");
    assert_eq!(at(0)["message"], "數量必須大於 0");
    assert_eq!(at(1)["field_key"], "unit_price");
    assert_eq!(at(2)["field_key"], "discount_rate");
}

/// 修改時依所有明細重新計算（只改表頭的含稅、稅別也會重算）。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn update_recalculates_amounts(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let so = order(
        &pool,
        &token,
        m.customer,
        vec![line(m.goods, "2", "105", "0")],
    )
    .await;
    assert_eq!(so.data["total"], "221.0000");

    // 改為含稅：明細不變，合計依含稅重算
    let (status, body) = patch(
        &pool,
        &token,
        so.id,
        json!({ "version": so.version, "data": { "tax_included": true } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["total"], "210.0000");
    assert_eq!(body["data"]["subtotal"], "200.0000");
    assert_eq!(body["data"]["tax_amount"], "10.0000");
    let version = body["version"].as_i64().unwrap();
    let line_id = body["lines"]["sales_order_line"][0]["id"].clone();

    // 改明細折扣、新增一列、改稅別
    let (status, body) = patch(
        &pool,
        &token,
        so.id,
        json!({
            "version": version,
            "data": { "tax_type": "zero_rated" },
            "lines": { "sales_order_line": [
                { "id": line_id, "data": { "discount_rate": "0.25" } },
                line(m.other, "1", "0.5", "0"),
            ] },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = &body["lines"]["sales_order_line"];
    // 2 × 105 × 0.75 = 157.5 → 158；0.5 → 1（MidpointAwayFromZero）
    assert_eq!(rows[0]["data"]["amount"], "158.0000");
    assert_eq!(rows[1]["data"]["amount"], "1.0000");
    assert_eq!(body["data"]["total"], "159.0000");
    assert_eq!(body["data"]["tax_amount"], "0.0000");

    // 沒有任何變動：version 不變
    let version = body["version"].as_i64().unwrap();
    let (status, body) = patch(
        &pool,
        &token,
        so.id,
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

    // 沒有明細 → 422
    let empty = order(&pool, &token, m.customer, vec![]).await;
    let (status, body) = action(&pool, &token, "sales_order", empty.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "VALIDATION");
    assert_eq!(body["details"]["lines"][0]["message"], "至少要有一筆明細");
    assert_eq!(body["details"]["lines"][0]["index"], Value::Null);

    let so = order(
        &pool,
        &token,
        m.customer,
        vec![line(m.goods, "2", "10", "0")],
    )
    .await;

    // 權限、不支援的動作、狀態、版本
    let (status, _) = action(&pool, &viewer, "sales_order", so.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = action(&pool, &token, "sales_order", so.id, "post", 1).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = action(&pool, &token, "sales_order", so.id, "close", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "INVALID_STATUS");
    let (status, _) = action(&pool, &token, "sales_order", so.id, "void", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = action(&pool, &token, "sales_order", so.id, "confirm", 9).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // 確認
    let (status, body) = action(&pool, &token, "sales_order", so.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "confirmed");
    assert_eq!(body["version"], 2);
    assert!(body["document"]["posted_at"].is_string());
    assert!(body["document"]["posted_by"].is_string());
    assert!(body["document"]["closed_at"].is_null());
    assert_eq!(body["data"]["total"], "21.0000");

    // 已確認：不可修改、刪除、再確認
    let (status, body) = patch(
        &pool,
        &token,
        so.id,
        json!({ "version": 2, "data": { "notes": "x" } }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "INVALID_STATUS");
    assert_eq!(body["details"], json!({ "status": "confirmed" }));
    let (status, _) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=2", record_uri("sales_order", so.id)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = action(&pool, &token, "sales_order", so.id, "confirm", 2).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "INVALID_STATUS");

    // 手動結案
    let (status, body) = action(&pool, &token, "sales_order", so.id, "close", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "closed");
    assert!(body["document"]["closed_at"].is_string());
    assert!(body["document"]["closed_by"].is_string());
    // 已結案不可作廢、再結案
    let (status, body) = action(&pool, &token, "sales_order", so.id, "void", 3).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["details"], json!({ "status": "closed" }));
    let (status, _) = action(&pool, &token, "sales_order", so.id, "close", 3).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        audit_actions(&pool, so.id).await,
        ["create", "confirm", "close"]
    );

    // 作廢：只有已確認
    let so2 = order(
        &pool,
        &token,
        m.customer,
        vec![line(m.goods, "1", "10", "0")],
    )
    .await;
    let (status, _) = action(&pool, &token, "sales_order", so2.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = action(&pool, &token, "sales_order", so2.id, "void", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "void");
    assert!(body["document"]["voided_at"].is_string());
    let (status, _) = action(&pool, &token, "sales_order", so2.id, "close", 3).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        audit_actions(&pool, so2.id).await,
        ["create", "confirm", "void"]
    );

    // 部分出貨：可以結案、不可作廢（出貨過帳在 #148，這裡直接設定狀態）
    let so3 = order(
        &pool,
        &token,
        m.customer,
        vec![line(m.goods, "1", "10", "0")],
    )
    .await;
    let (status, _) = action(&pool, &token, "sales_order", so3.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE sales_orders SET status = 'partially_shipped' WHERE id = $1")
        .bind(so3.id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = action(&pool, &token, "sales_order", so3.id, "void", 2).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["message"], "部分出貨的單據不可作廢");
    let (status, body) = action(&pool, &token, "sales_order", so3.id, "close", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "closed");
}

/// 確認時客戶、商品已刪除 → 422，整張 rollback。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn confirm_rejects_deleted_references(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let so = order(
        &pool,
        &token,
        m.customer,
        vec![line(m.goods, "1", "10", "0"), line(m.other, "1", "10", "0")],
    )
    .await;
    // 草稿引用的商品可以刪除（只限制未結案的訂單）
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("product", m.other)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    sqlx::query("UPDATE customers SET deleted_at = now() WHERE id = $1")
        .bind(m.customer)
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = action(&pool, &token, "sales_order", so.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "customer_id", "message": "客戶已刪除" }])
    );
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "sales_order_line", "index": 1, "field_key": "product_id",
                 "message": "商品已刪除" }])
    );
    let (_, body) = call(
        &pool,
        Method::GET,
        &record_uri("sales_order", so.id),
        &token,
        None,
    )
    .await;
    assert_eq!(body["data"]["status"], "draft");
    assert_eq!(body["version"], 1);
}

/// 客戶有銷貨單據不可刪除；商品有未結案的訂單不可刪除、改為服務、修改單位。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn referenced_customer_and_product_restrictions(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let so = order(
        &pool,
        &token,
        m.customer,
        vec![line(m.goods, "1", "10", "0")],
    )
    .await;

    // 客戶：草稿也算
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("customer", m.customer)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["message"],
        "此客戶已有 1 張訂單或出貨單據，無法刪除，請改為停用"
    );
    assert_eq!(
        body["details"],
        json!({ "fields": [], "document_count": 1 })
    );
    // 作廢的訂單也算
    let so2 = order(
        &pool,
        &token,
        m.other_customer,
        vec![line(m.other, "1", "10", "0")],
    )
    .await;
    action(&pool, &token, "sales_order", so2.id, "confirm", 1).await;
    let (status, _) = action(&pool, &token, "sales_order", so2.id, "void", 2).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("customer", m.other_customer)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // 沒有單據的客戶可以刪除
    let c3 = create(
        &pool,
        &token,
        "customer",
        json!({ "data": { "code": "C003", "name": "客戶 C003" } }),
    )
    .await;
    let (status, _) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("customer", c3.id)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 商品：草稿不限制、確認後限制
    let (status, _) = action(&pool, &token, "sales_order", so.id, "confirm", 1).await;
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
    assert_eq!(body["message"], "此商品有 1 張未結案的訂單，無法刪除");
    assert_eq!(
        body["details"],
        json!({ "fields": [], "open_sales_order_count": 1 })
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
            { "field_key": "product_type", "message": "此商品有未結案的訂單，無法改為服務" },
            { "field_key": "unit_id", "message": "此商品有未結案的訂單，無法修改單位" },
        ])
    );

    // 同時有未結案的採購單：兩個數量都列出，訊息為採購單
    let supplier = create(
        &pool,
        &token,
        "supplier",
        json!({ "data": { "code": "S001", "name": "廠商", "default_currency_code": "TWD" } }),
    )
    .await;
    let po = create(
        &pool,
        &token,
        "purchase_order",
        json!({
            "data": { "supplier_id": supplier.id, "order_date": "2026-11-18" },
            "lines": { "purchase_order_line": [
                { "data": { "product_id": m.goods, "qty": "1", "unit_price": "1" } },
            ] },
        }),
    )
    .await;
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
        json!({ "fields": [], "open_purchase_order_count": 1, "open_sales_order_count": 1 })
    );
    let (status, _) = action(&pool, &token, "purchase_order", po.id, "close", 2).await;
    assert_eq!(status, StatusCode::OK);

    // 結案後不再限制
    let (status, _) = action(&pool, &token, "sales_order", so.id, "close", 2).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("product", m.goods)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}
