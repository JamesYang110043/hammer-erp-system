//! 出貨單的整合測試（#148，docs/architecture.md §8.9「出貨過帳與作廢」、§9.2、§9.3）：先進先出跨批號、
//! 手動指定批號、庫存不足、訂單分批出貨的狀態、作廢回復、並行出貨同一商品。
//!
//! 使用 `#[sqlx::test]`，需要 `DATABASE_URL`（見 README「整合測試」）。
//! 透過完整的 router 呼叫，包含所有中介層。

#![allow(clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use hammer_core::test_util::{TestUser, create_user_with_session};
use hammer_server::{AppState, Config, router};
use hammer_shared::decimal::Decimal;
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
    goods: Id,
    other: Id,
    service: Id,
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
    let product = |code: &str, product_type: &str| {
        json!({ "data": { "code": code, "name": format!("商品 {code}"), "unit_id": unit,
                          "product_type": product_type } })
    };
    let goods = create(pool, token, "product", product("A001", "goods"))
        .await
        .id;
    let other = create(pool, token, "product", product("A002", "goods"))
        .await
        .id;
    let service = create(pool, token, "product", product("FEE", "service"))
        .await
        .id;
    Master {
        customer: c1,
        other_customer: c2,
        goods,
        other,
        service,
    }
}

/// 以期初庫存的盤點調整單建立批號（過帳）：`lots` 為 `(商品, 批號, 數量, 單位成本)`。
async fn stock(pool: &PgPool, token: &str, date: &str, lots: &[(Id, &str, &str, &str)]) {
    let lines: Vec<Value> = lots
        .iter()
        .map(|(product, lot_no, qty, cost)| {
            json!({ "data": { "line_type": "gain_new", "product_id": product, "lot_no": lot_no,
                              "qty": qty, "unit_cost_twd": cost } })
        })
        .collect();
    let adj = create(
        pool,
        token,
        "stock_adjustment",
        json!({
            "data": { "adjustment_date": date, "adjustment_type": "opening" },
            "lines": { "stock_adjustment_line": lines },
        }),
    )
    .await;
    let (status, body) = action(pool, token, "stock_adjustment", adj.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 商品的批號 `(批號, 剩餘數量)`，依先進先出順序。
async fn lots(pool: &PgPool, product: Id) -> Vec<(String, String)> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT lot_no, qty_remaining::text FROM inventory_lots
         WHERE product_id = $1 AND deleted_at IS NULL ORDER BY received_date, id",
    )
    .bind(product)
    .fetch_all(pool)
    .await
    .unwrap();
    rows
}

fn lot_rows(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

/// 出貨單的異動 `(批號, 異動類型, 數量, 成本)`，依建立順序。
async fn transactions(pool: &PgPool, shipment: Id) -> Vec<(String, String, String, String)> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT l.lot_no, t.txn_type, t.qty_change::text, t.unit_cost_twd::text
         FROM inventory_transactions t JOIN inventory_lots l ON l.id = t.lot_id
         WHERE t.ref_doc_id = $1 ORDER BY t.id",
    )
    .bind(shipment)
    .fetch_all(pool)
    .await
    .unwrap();
    rows
}

fn line(product: Id, qty: &str, price: &str) -> Value {
    json!({ "data": { "product_id": product, "qty": qty, "unit_price": price } })
}

fn lot_line(product: Id, qty: &str, lot_no: &str) -> Value {
    json!({ "data": { "product_id": product, "qty": qty, "unit_price": "100", "lot_no": lot_no } })
}

async fn shipment(pool: &PgPool, token: &str, customer: Id, lines: Vec<Value>) -> Record {
    create(
        pool,
        token,
        "shipment",
        json!({
            "data": { "customer_id": customer, "ship_date": "2026-11-18" },
            "lines": { "shipment_line": lines },
        }),
    )
    .await
}

fn shipment_lines(body: &Value) -> &Vec<Value> {
    body["lines"]["shipment_line"].as_array().unwrap()
}

/// 先進先出跨多個批號、銷貨成本；服務類商品不扣庫存；作廢後完全回復。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn fifo_across_lots_cost_and_void(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(
        &pool,
        &token,
        "2026-11-01",
        &[(m.goods, "L-02", "7", "82.5")],
    )
    .await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;

    let sh = shipment(
        &pool,
        &token,
        m.customer,
        vec![line(m.goods, "12", "100"), line(m.service, "1", "50")],
    )
    .await;
    // 存檔時計算金額，草稿沒有成本
    assert_eq!(sh.data["tax_type"], "taxable");
    assert_eq!(sh.data["subtotal"], "1250.0000");
    assert!(!sh.data.contains_key("cost_total_twd"));

    let (status, body) = action(&pool, &token, "shipment", sh.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "posted");
    // 5 × 80 + 7 × 82.5 = 977.5
    assert_eq!(body["data"]["cost_total_twd"], "977.50");
    let rows = shipment_lines(&body);
    assert_eq!(rows[0]["data"]["cost_amount_twd"], "977.50");
    assert!(rows[1]["data"].get("cost_amount_twd").is_none(), "{body}");
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "0.0000"), ("L-02", "0.0000")])
    );
    assert_eq!(
        transactions(&pool, sh.id).await,
        [
            (
                "L-01".into(),
                "shipment".into(),
                "-5.0000".into(),
                "80.0000".into()
            ),
            (
                "L-02".into(),
                "shipment".into(),
                "-7.0000".into(),
                "82.5000".into()
            ),
        ]
    );
    // 先進先出跨批號時明細沒有 lot_id
    let lot_id: Option<Id> = sqlx::query_scalar("SELECT lot_id FROM shipment_lines WHERE id = $1")
        .bind(sh.lines.as_ref().unwrap()["shipment_line"][0].id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(lot_id.is_none());

    // 作廢：數量加回原批號，成本保留
    let (status, body) = action(&pool, &token, "shipment", sh.id, "void", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "void");
    assert_eq!(body["data"]["cost_total_twd"], "977.50");
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "5.0000"), ("L-02", "7.0000")])
    );
    let txns = transactions(&pool, sh.id).await;
    assert_eq!(txns.len(), 4);
    assert_eq!(
        txns[2..],
        [
            (
                "L-01".into(),
                "void_reversal".into(),
                "5.0000".into(),
                "80.0000".into()
            ),
            (
                "L-02".into(),
                "void_reversal".into(),
                "7.0000".into(),
                "82.5000".into()
            ),
        ]
    );
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(sh.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actions, ["create", "post", "void"]);
}

/// 手動指定批號的列先扣，之後才是先進先出；指定批號的列寫入 `lot_id`。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn manual_lots_are_issued_before_fifo(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;
    stock(
        &pool,
        &token,
        "2026-11-01",
        &[(m.goods, "L-02", "7", "82.5")],
    )
    .await;

    // 第 1 列先進先出 3、第 2 列指定 L-01（小寫、前後空白）4
    let sh = shipment(
        &pool,
        &token,
        m.customer,
        vec![line(m.goods, "3", "100"), lot_line(m.goods, "4", "  l-01 ")],
    )
    .await;
    assert_eq!(
        sh.lines.as_ref().unwrap()["shipment_line"][1].data["lot_no"],
        "l-01"
    );
    let (status, body) = action(&pool, &token, "shipment", sh.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = shipment_lines(&body);
    // L-01 先扣 4 剩 1；先進先出：L-01 1 + L-02 2 = 80 + 165
    assert_eq!(rows[0]["data"]["cost_amount_twd"], "245.00");
    assert_eq!(rows[1]["data"]["cost_amount_twd"], "320.00");
    assert_eq!(body["data"]["cost_total_twd"], "565.00");
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "0.0000"), ("L-02", "5.0000")])
    );
    let lot_ids: Vec<Option<Id>> = sqlx::query_scalar(
        "SELECT lot_id FROM shipment_lines WHERE shipment_id = $1 ORDER BY sort_order",
    )
    .bind(sh.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(lot_ids[0].is_none());
    assert!(lot_ids[1].is_some());
}

/// 庫存不足：整張失敗、列出所有不足（指定批號一個批號一項、先進先出一個商品一項），庫存不變。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn stock_insufficient_rolls_back(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;

    let sh = shipment(
        &pool,
        &token,
        m.customer,
        vec![
            lot_line(m.goods, "6", "L-01"),
            line(m.other, "2", "10"),
            line(m.other, "1", "10"),
        ],
    )
    .await;
    let (status, body) = action(&pool, &token, "shipment", sh.id, "post", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "STOCK_INSUFFICIENT");
    assert_eq!(
        body["message"],
        "商品 A001 批號 L-01 庫存不足，尚缺 1 等 2 項"
    );
    let items = body["details"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["lot_no"], "L-01");
    assert_eq!(items[0]["required"], "6.0000");
    assert_eq!(items[0]["available"], "5.0000");
    assert_eq!(items[1]["product_code"], "A002");
    assert!(items[1]["lot_no"].is_null());
    assert_eq!(items[1]["required"], "3.0000");
    assert_eq!(items[1]["shortage"], "3.0000");
    assert_eq!(
        items[1]["lines"],
        json!([{ "detail_key": "shipment_line", "index": 1 },
               { "detail_key": "shipment_line", "index": 2 }])
    );
    assert_eq!(lots(&pool, m.goods).await, lot_rows(&[("L-01", "5.0000")]));
    assert!(transactions(&pool, sh.id).await.is_empty());
    let (_, body) = call(
        &pool,
        Method::GET,
        &record_uri("shipment", sh.id),
        &token,
        None,
    )
    .await;
    assert_eq!(body["data"]["status"], "draft");
    assert_eq!(body["version"], 1);
}

/// 存檔時的檢查：批號、服務類商品的批號、沒有訂單的訂單明細。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn save_validates_lots_and_order_lines(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;
    let so = create(
        &pool,
        &token,
        "sales_order",
        json!({
            "data": { "customer_id": m.customer, "order_date": "2026-11-18" },
            "lines": { "sales_order_line": [line(m.goods, "1", "1")] },
        }),
    )
    .await;
    let so_line = so.lines.as_ref().unwrap()["sales_order_line"][0].id;

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("shipment"),
        &token,
        Some(json!({
            "data": { "customer_id": m.customer, "ship_date": "2026-11-18" },
            "lines": { "shipment_line": [
                lot_line(m.goods, "1", "NOPE"),
                lot_line(m.other, "1", "L-01"),
                lot_line(m.service, "1", "X"),
                { "data": { "product_id": m.goods, "qty": "1", "unit_price": "1",
                            "sales_order_line_id": so_line } },
            ] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let errors: Vec<(i64, String, String)> = body["details"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["index"].as_i64().unwrap(),
                e["field_key"].as_str().unwrap().to_owned(),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        errors,
        [
            (0, "lot_no".into(), "找不到此批號".into()),
            (1, "lot_no".into(), "找不到此批號".into()),
            (2, "lot_no".into(), "服務類商品沒有批號".into()),
            (3, "sales_order_line_id".into(), "沒有指定訂單".into()),
        ]
    );
}

/// 訂單轉出貨單：稅別帶入訂單的值；同一張訂單分批出貨，狀態依序變化；作廢後訂單回復。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn partial_shipments_update_order(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(
        &pool,
        &token,
        "2026-10-02",
        &[(m.goods, "L-01", "20", "80")],
    )
    .await;
    let so = create(
        &pool,
        &token,
        "sales_order",
        json!({
            "data": { "customer_id": m.customer, "order_date": "2026-11-18",
                      "tax_type": "zero_rated" },
            "lines": { "sales_order_line": [line(m.goods, "10", "100"), line(m.service, "1", "50")] },
        }),
    )
    .await;
    let so_lines = &so.lines.as_ref().unwrap()["sales_order_line"];
    let (goods_line, fee_line) = (so_lines[0].id, so_lines[1].id);
    let from_order = |qty: &str, customer: Id| {
        json!({
            "data": { "customer_id": customer, "ship_date": "2026-11-18",
                      "sales_order_id": so.id },
            "lines": { "shipment_line": [
                { "data": { "product_id": m.goods, "qty": qty, "unit_price": "100",
                            "sales_order_line_id": goods_line } },
            ] },
        })
    };

    // 訂單還是草稿
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("shipment"),
        &token,
        Some(from_order("4", m.customer)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "sales_order_id", "message": "訂單尚未確認" }])
    );
    let (status, _) = action(&pool, &token, "sales_order", so.id, "confirm", 1).await;
    assert_eq!(status, StatusCode::OK);

    // 客戶不同
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("shipment"),
        &token,
        Some(from_order("4", m.other_customer)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "customer_id", "message": "客戶與訂單不同" }])
    );

    // 第一批 4：稅別帶入訂單的零稅率
    let sh1 = create(&pool, &token, "shipment", from_order("4", m.customer)).await;
    assert_eq!(sh1.data["tax_type"], "zero_rated");
    let (status, body) = action(&pool, &token, "shipment", sh1.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let order = |pool: PgPool, token: String| async move {
        let (_, body) = call(
            &pool,
            Method::GET,
            &record_uri("sales_order", so.id),
            &token,
            None,
        )
        .await;
        body
    };
    let o = order(pool.clone(), token.clone()).await;
    assert_eq!(o["data"]["status"], "partially_shipped");
    assert_eq!(
        o["lines"]["sales_order_line"][0]["data"]["shipped_qty"],
        "4.0000"
    );

    // 超過未出貨數量（草稿不保留數量，只算已過帳）
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("shipment"),
        &token,
        Some(from_order("7", m.customer)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"][0]["message"],
        "超過未出貨數量 6.0000"
    );

    // 第二批：剩下的商品 6 與服務 1 → 全部出貨，自動結案
    let sh2 = create(
        &pool,
        &token,
        "shipment",
        json!({
            "data": { "customer_id": m.customer, "ship_date": "2026-11-19",
                      "sales_order_id": so.id },
            "lines": { "shipment_line": [
                { "data": { "product_id": m.goods, "qty": "6", "unit_price": "100",
                            "sales_order_line_id": goods_line } },
                { "data": { "product_id": m.service, "qty": "1", "unit_price": "50",
                            "sales_order_line_id": fee_line } },
            ] },
        }),
    )
    .await;
    let (status, body) = action(&pool, &token, "shipment", sh2.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let o = order(pool.clone(), token.clone()).await;
    assert_eq!(o["data"]["status"], "closed");
    assert!(o["document"]["closed_at"].is_null());

    // 作廢第二批 → 部分出貨；作廢第一批 → 已確認
    let (status, body) = action(&pool, &token, "shipment", sh2.id, "void", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let o = order(pool.clone(), token.clone()).await;
    assert_eq!(o["data"]["status"], "partially_shipped");
    assert_eq!(
        o["lines"]["sales_order_line"][1]["data"]["shipped_qty"],
        "0.0000"
    );
    let (status, _) = action(&pool, &token, "shipment", sh1.id, "void", 2).await;
    assert_eq!(status, StatusCode::OK);
    let o = order(pool.clone(), token.clone()).await;
    assert_eq!(o["data"]["status"], "confirmed");
    assert_eq!(
        o["lines"]["sales_order_line"][0]["data"]["shipped_qty"],
        "0.0000"
    );
    assert_eq!(lots(&pool, m.goods).await, lot_rows(&[("L-01", "20.0000")]));
    // 訂單的操作紀錄：每次出貨過帳、作廢各一筆 update
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(so.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        actions,
        ["create", "confirm", "update", "update", "update", "update"]
    );

    // 手動結案的訂單：出貨過帳被拒
    let sh3 = create(&pool, &token, "shipment", from_order("1", m.customer)).await;
    let version = o["version"].as_i64().unwrap() as i32;
    let (status, _) = action(&pool, &token, "sales_order", so.id, "close", version).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = action(&pool, &token, "shipment", sh3.id, "post", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "sales_order_id", "message": "訂單已結案" }])
    );
}

/// 已有過帳的銷貨退回時不可作廢。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn void_blocked_by_posted_return(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;
    let sh = shipment(&pool, &token, m.customer, vec![line(m.goods, "2", "100")]).await;
    let (status, _) = action(&pool, &token, "shipment", sh.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK);
    let sr = create(
        &pool,
        &token,
        "sales_return",
        json!({
            "data": { "shipment_id": sh.id, "return_date": "2026-11-19" },
            "lines": { "sales_return_line": [
                { "data": { "shipment_line_id": sh.lines.as_ref().unwrap()["shipment_line"][0].id,
                            "qty": "1" } },
            ] },
        }),
    )
    .await;
    // 銷貨退回的過帳在 #150，這裡直接設定狀態
    sqlx::query("UPDATE sales_returns SET status = 'posted', posted_at = now(), posted_by = created_by WHERE id = $1")
        .bind(sr.id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = action(&pool, &token, "shipment", sh.id, "void", 2).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let no = sr.record_no.unwrap();
    assert_eq!(
        body["message"],
        format!("此出貨單已有銷貨退回 {no}，請先作廢退回單")
    );
    assert_eq!(body["details"]["documents"][0]["record_no"], no);
    assert_eq!(lots(&pool, m.goods).await, lot_rows(&[("L-01", "3.0000")]));
}

/// 並行出貨同一商品：批號鎖定讓兩張依序過帳，庫存只夠一張時另一張 STOCK_INSUFFICIENT。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn concurrent_shipments_of_same_product(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "3", "80")]).await;
    stock(&pool, &token, "2026-11-01", &[(m.goods, "L-02", "2", "90")]).await;
    let a = shipment(&pool, &token, m.customer, vec![line(m.goods, "4", "100")]).await;
    let b = shipment(
        &pool,
        &token,
        m.customer,
        vec![lot_line(m.goods, "2", "L-02")],
    )
    .await;
    let c = shipment(&pool, &token, m.customer, vec![line(m.goods, "4", "100")]).await;

    let (ra, rb, rc) = tokio::join!(
        action(&pool, &token, "shipment", a.id, "post", 1),
        action(&pool, &token, "shipment", b.id, "post", 1),
        action(&pool, &token, "shipment", c.id, "post", 1),
    );
    let ok: Vec<bool> = [&ra, &rb, &rc]
        .iter()
        .map(|(s, body)| {
            assert!(
                *s == StatusCode::OK
                    || (*s == StatusCode::UNPROCESSABLE_ENTITY
                        && body["code"] == "STOCK_INSUFFICIENT"),
                "{s} {body}"
            );
            *s == StatusCode::OK
        })
        .collect();
    // 總庫存 5：a、c 各要 4 不可能都成功；扣除的合計不超過 5，且與成功的單據一致
    assert!(!(ok[0] && ok[2]));
    let issued: Decimal = [(ok[0], 4), (ok[1], 2), (ok[2], 4)]
        .iter()
        .filter(|(o, _)| *o)
        .map(|(_, q)| Decimal::from(*q))
        .sum();
    let remaining: Decimal =
        sqlx::query_scalar("SELECT sum(qty_remaining) FROM inventory_lots WHERE product_id = $1")
            .bind(m.goods)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, Decimal::from(5) - issued);
    assert!(remaining >= Decimal::ZERO);
    assert!(ok.iter().any(|o| *o));
}
