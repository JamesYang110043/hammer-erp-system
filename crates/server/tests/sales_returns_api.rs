//! 銷貨退回的整合測試（#150，docs/architecture.md §8.9「銷貨退回」、§9.3）：退回數量上限、跨多批號的出貨
//! 部分退回（加回的批號與成本）、作廢與加回的貨已被再次扣用。
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
    goods: Id,
    service: Id,
}

async fn master_data(pool: &PgPool, token: &str) -> Master {
    let customer = |code: &str| json!({ "data": { "code": code, "name": format!("客戶 {code}") } });
    let c1 = create(pool, token, "customer", customer("C001")).await.id;
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
    let service = create(pool, token, "product", product("FEE", "service"))
        .await
        .id;
    Master {
        customer: c1,
        goods,
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

/// 單據的異動 `(批號, 異動類型, 數量, 成本)`，依建立順序。
async fn transactions(pool: &PgPool, doc: Id) -> Vec<(String, String, String, String)> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT l.lot_no, t.txn_type, t.qty_change::text, t.unit_cost_twd::text
         FROM inventory_transactions t JOIN inventory_lots l ON l.id = t.lot_id
         WHERE t.ref_doc_id = $1 ORDER BY t.id",
    )
    .bind(doc)
    .fetch_all(pool)
    .await
    .unwrap();
    rows
}

fn line(product: Id, qty: &str, price: &str) -> Value {
    json!({ "data": { "product_id": product, "qty": qty, "unit_price": price } })
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

/// 已過帳的出貨單：A001 先進先出 12（L-01 扣 5 @80、L-02 扣 7 @82.5）、運費 1。
async fn posted_shipment(pool: &PgPool, token: &str, m: &Master) -> (Id, Id, Id) {
    stock(pool, token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;
    stock(pool, token, "2026-11-01", &[(m.goods, "L-02", "7", "82.5")]).await;
    let sh = shipment(
        pool,
        token,
        m.customer,
        vec![line(m.goods, "12", "105"), line(m.service, "1", "50")],
    )
    .await;
    let (status, body) = action(pool, token, "shipment", sh.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = &sh.lines.as_ref().unwrap()["shipment_line"];
    (sh.id, rows[0].id, rows[1].id)
}

fn return_body(shipment: Id, date: &str, lines: &[(Id, &str)]) -> Value {
    let lines: Vec<Value> = lines
        .iter()
        .map(|(id, qty)| json!({ "data": { "shipment_line_id": id, "qty": qty } }))
        .collect();
    json!({
        "data": { "shipment_id": shipment, "return_date": date },
        "lines": { "sales_return_line": lines },
    })
}

/// 跨多批號的出貨部分退回：依扣除的反向順序加回原批號，成本用原批號的成本；金額由出貨明細帶入計算。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn partial_returns_refill_original_lots(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (sh, goods_line, fee_line) = posted_shipment(&pool, &token, &m).await;
    // 出貨後再進一批較新的批號：退回不會加到它
    stock(&pool, &token, "2026-11-10", &[(m.goods, "L-03", "1", "90")]).await;

    // 第一次退 8 與運費 → L-02 加回 7、L-01 加回 1
    let sr1 = create(
        &pool,
        &token,
        "sales_return",
        return_body(sh, "2026-11-19", &[(goods_line, "8"), (fee_line, "1")]),
    )
    .await;
    // 金額：8 × 105 + 50 = 890；應稅 5% 未稅 → 稅額 round(44.5) = 45
    assert_eq!(sr1.data["subtotal"], "890.0000");
    assert_eq!(sr1.data["tax_amount"], "45.0000");
    assert_eq!(sr1.data["total"], "935.0000");
    let (status, body) = action(&pool, &token, "sales_return", sr1.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "posted");
    // 7 × 82.5 + 1 × 80 = 657.5
    assert_eq!(body["data"]["cost_total_twd"], "657.50");
    let rows = body["lines"]["sales_return_line"].as_array().unwrap();
    assert_eq!(rows[0]["data"]["cost_amount_twd"], "657.50");
    assert!(rows[1]["data"].get("cost_amount_twd").is_none(), "{body}");
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "1.0000"), ("L-02", "7.0000"), ("L-03", "1.0000")])
    );
    assert_eq!(
        transactions(&pool, sr1.id).await,
        [
            (
                "L-02".into(),
                "sales_return".into(),
                "7.0000".into(),
                "82.5000".into()
            ),
            (
                "L-01".into(),
                "sales_return".into(),
                "1.0000".into(),
                "80.0000".into()
            ),
        ]
    );

    // 超過可退數量（12 − 8 = 4）
    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_return"),
        &token,
        Some(return_body(sh, "2026-11-19", &[(goods_line, "5")])),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "sales_return_line", "index": 0, "field_key": "qty",
                 "message": "超過可退數量 4.0000" }])
    );

    // 第二次退 4 → L-01 加回 4
    let sr2 = create(
        &pool,
        &token,
        "sales_return",
        return_body(sh, "2026-11-20", &[(goods_line, "4")]),
    )
    .await;
    let (status, body) = action(&pool, &token, "sales_return", sr2.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["cost_total_twd"], "320.00");
    assert_eq!(
        transactions(&pool, sr2.id).await,
        [(
            "L-01".into(),
            "sales_return".into(),
            "4.0000".into(),
            "80.0000".into()
        )]
    );
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "5.0000"), ("L-02", "7.0000"), ("L-03", "1.0000")])
    );

    // 作廢第二次：扣回 L-01 4；之後又可以退 4
    let (status, body) = action(&pool, &token, "sales_return", sr2.id, "void", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["cost_total_twd"], "320.00");
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "1.0000"), ("L-02", "7.0000"), ("L-03", "1.0000")])
    );
    assert_eq!(
        transactions(&pool, sr2.id).await[1],
        (
            "L-01".into(),
            "void_reversal".into(),
            "-4.0000".into(),
            "80.0000".into()
        )
    );
    let sr3 = create(
        &pool,
        &token,
        "sales_return",
        return_body(sh, "2026-11-21", &[(goods_line, "4")]),
    )
    .await;
    let (status, body) = action(&pool, &token, "sales_return", sr3.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 作廢全部退回後出貨單可以作廢，庫存回到出貨前
    for (id, version) in [(sr1.id, 2), (sr3.id, 2)] {
        let (status, body) = action(&pool, &token, "sales_return", id, "void", version).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body) = action(&pool, &token, "shipment", sh, "void", 2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "5.0000"), ("L-02", "7.0000"), ("L-03", "1.0000")])
    );
}

/// 存檔時的檢查：退回日期、出貨明細重複；過帳時以最新的已過帳退回重新檢查數量上限。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn limits_are_checked_on_save_and_post(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (sh, goods_line, _) = posted_shipment(&pool, &token, &m).await;

    let (status, body) = call(
        &pool,
        Method::POST,
        &records_uri("sales_return"),
        &token,
        Some(return_body(
            sh,
            "2026-11-17",
            &[(goods_line, "1"), (goods_line, "1")],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "return_date", "message": "退回日期不可早於出貨日期" }])
    );
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": "sales_return_line", "index": 1,
                 "field_key": "shipment_line_id", "message": "出貨明細重複" }])
    );

    // 兩張草稿各退 8（草稿不保留數量，都可以存檔）；第一張過帳後第二張超過
    let a = create(
        &pool,
        &token,
        "sales_return",
        return_body(sh, "2026-11-18", &[(goods_line, "8")]),
    )
    .await;
    let b = create(
        &pool,
        &token,
        "sales_return",
        return_body(sh, "2026-11-18", &[(goods_line, "8")]),
    )
    .await;
    let (status, _) = action(&pool, &token, "sales_return", a.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = action(&pool, &token, "sales_return", b.id, "post", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"][0]["message"],
        "超過可退數量 4.0000"
    );
    assert!(transactions(&pool, b.id).await.is_empty());
}

/// 加回的數量已被再次出貨時，作廢退回 → STOCK_INSUFFICIENT（一個批號一項），整張 rollback。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn void_rejected_when_returned_stock_was_used(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let (sh, goods_line, _) = posted_shipment(&pool, &token, &m).await;
    let sr = create(
        &pool,
        &token,
        "sales_return",
        return_body(sh, "2026-11-19", &[(goods_line, "8")]),
    )
    .await;
    let (status, _) = action(&pool, &token, "sales_return", sr.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK);

    // 加回的 L-02 7 被再次出貨 6
    let again = shipment(
        &pool,
        &token,
        m.customer,
        vec![
            json!({ "data": { "product_id": m.goods, "qty": "6", "unit_price": "1",
                               "lot_no": "L-02" } }),
        ],
    )
    .await;
    let (status, body) = action(&pool, &token, "shipment", again.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = action(&pool, &token, "sales_return", sr.id, "void", 2).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "STOCK_INSUFFICIENT");
    let items = body["details"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["lot_no"], "L-02");
    assert_eq!(items[0]["required"], "7.0000");
    assert_eq!(items[0]["shortage"], "6.0000");
    assert_eq!(
        items[0]["lines"],
        json!([{ "detail_key": "sales_return_line", "index": 0 }])
    );
    // rollback：L-01 也沒有扣回
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "1.0000"), ("L-02", "1.0000")])
    );
    let (_, body) = call(
        &pool,
        Method::GET,
        &record_uri("sales_return", sr.id),
        &token,
        None,
    )
    .await;
    assert_eq!(body["data"]["status"], "posted");
}
