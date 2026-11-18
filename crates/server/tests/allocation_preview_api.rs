//! 建議批號 API 的整合測試（#149，docs/architecture.md §8.9「建議批號 API」）：建議結果與實際過帳的扣減
//! 一致、不足時回傳缺少的數量、格式與參照錯誤、只試算不寫入。
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
    other: Id,
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
    let other = create(pool, token, "product", product("A002", "goods"))
        .await
        .id;
    let service = create(pool, token, "product", product("FEE", "service"))
        .await
        .id;
    Master {
        customer: c1,
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

const PREVIEW: &str = "/api/v1/inventory/allocation-preview";

async fn preview(pool: &PgPool, token: &str, lines: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        PREVIEW,
        token,
        Some(json!({ "lines": lines })),
    )
    .await
}

/// 出貨單實際扣除的批號 `(明細位置, 批號, 數量)`，依扣除順序。
async fn posted_allocations(pool: &PgPool, shipment: Id) -> Vec<(i32, String, String)> {
    sqlx::query_as(
        "SELECT sl.sort_order, l.lot_no, a.qty::text
         FROM lot_allocations a
         JOIN shipment_lines sl ON sl.id = a.source_line_id
         JOIN inventory_lots l ON l.id = a.lot_id
         WHERE a.source_doc_id = $1 ORDER BY sl.sort_order, a.id",
    )
    .bind(shipment)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// 建議結果的 `(位置, 批號, 數量)`。
fn previewed(body: &Value) -> Vec<(i32, String, String)> {
    let mut out = Vec::new();
    for line in body["lines"].as_array().unwrap() {
        for a in line["allocations"].as_array().unwrap() {
            out.push((
                i32::try_from(line["index"].as_i64().unwrap()).unwrap(),
                a["lot_no"].as_str().unwrap().to_owned(),
                a["qty"].as_str().unwrap().to_owned(),
            ));
        }
    }
    out
}

/// 建議結果與同一個庫存狀態下實際過帳的扣減結果一致；試算不寫入任何資料。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn preview_matches_posting(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let viewer = user(&pool, "viewer", role::VIEWER).await;
    let m = master_data(&pool, &token).await;
    stock(
        &pool,
        &token,
        "2026-11-01",
        &[(m.goods, "L-02", "7", "82.5")],
    )
    .await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;
    stock(&pool, &token, "2026-10-05", &[(m.other, "B-01", "4", "10")]).await;

    // 先進先出 3、指定 L-01 4、先進先出 6（接著扣）、服務、其他商品
    let lines = json!([
        { "product_id": m.goods, "qty": "3" },
        { "product_id": m.goods, "qty": "4", "lot_no": " l-01 " },
        { "product_id": m.goods, "qty": "5" },
        { "product_id": m.service, "qty": "1" },
        { "product_id": m.other, "qty": "4", "lot_no": "" },
    ]);
    // 只需要 record.read
    let (status, body) = preview(&pool, &viewer, lines.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["lines"].as_array().unwrap();
    assert_eq!(rows[0]["product_type"], "goods");
    assert_eq!(rows[0]["qty"], "3.0000");
    assert_eq!(
        rows[0]["allocations"],
        json!([
            { "lot_id": rows[0]["allocations"][0]["lot_id"], "lot_no": "L-01",
              "received_date": "2026-10-02", "qty": "1.0000", "unit_cost_twd": "80.0000" },
            { "lot_id": rows[0]["allocations"][1]["lot_id"], "lot_no": "L-02",
              "received_date": "2026-11-01", "qty": "2.0000", "unit_cost_twd": "82.5000" },
        ])
    );
    assert_eq!(rows[2]["shortage"], "0.0000");
    assert_eq!(rows[3]["product_type"], "service");
    assert_eq!(rows[3]["allocations"], json!([]));
    assert_eq!(rows[3]["shortage"], "0.0000");
    // 試算不寫入
    assert_eq!(
        lots(&pool, m.goods).await,
        lot_rows(&[("L-01", "5.0000"), ("L-02", "7.0000")])
    );

    // 同樣的明細建立出貨單並過帳：扣除的批號與數量相同
    let sh = shipment(
        &pool,
        &token,
        m.customer,
        vec![
            line(m.goods, "3", "1"),
            lot_line(m.goods, "4", "l-01"),
            line(m.goods, "5", "1"),
            line(m.service, "1", "1"),
            line(m.other, "4", "1"),
        ],
    )
    .await;
    let (status, posted) = action(&pool, &token, "shipment", sh.id, "post", 1).await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    assert_eq!(posted_allocations(&pool, sh.id).await, previewed(&body));
}

/// 不足時回傳能分配的部分與不足的數量，不是錯誤；同一個狀態下過帳回傳 STOCK_INSUFFICIENT。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn shortage_is_not_an_error(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;

    let (status, body) = preview(
        &pool,
        &token,
        json!([
            { "product_id": m.goods, "qty": "8" },
            { "product_id": m.other, "qty": "2.5" },
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["lines"].as_array().unwrap();
    assert_eq!(rows[0]["allocations"][0]["qty"], "5.0000");
    assert_eq!(rows[0]["shortage"], "3.0000");
    assert_eq!(rows[1]["allocations"], json!([]));
    assert_eq!(rows[1]["shortage"], "2.5000");

    let sh = shipment(&pool, &token, m.customer, vec![line(m.goods, "8", "1")]).await;
    let (status, body) = action(&pool, &token, "shipment", sh.id, "post", 1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["details"]["items"][0]["shortage"], "3.0000");
}

/// 格式錯誤、商品不存在、批號找不到 → 422，`detail_key` 為 null。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn invalid_lines_are_rejected(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    stock(&pool, &token, "2026-10-02", &[(m.goods, "L-01", "5", "80")]).await;

    let (status, body) = preview(&pool, &token, json!([])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([{ "detail_key": null, "index": null, "field_key": null,
                 "message": "明細必須有 1 到 200 列" }])
    );
    let many: Vec<Value> = (0..201)
        .map(|_| json!({ "product_id": m.goods, "qty": "1" }))
        .collect();
    let (status, _) = preview(&pool, &token, Value::Array(many)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, body) = preview(
        &pool,
        &token,
        json!([
            { "qty": "1" },
            { "product_id": "x", "qty": "abc" },
            { "product_id": m.goods, "qty": "0" },
            { "product_id": m.goods, "qty": "1.00001" },
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let errors: Vec<(i64, String, String)> = body["details"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            assert!(e["detail_key"].is_null(), "{e}");
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
            (0, "product_id".into(), "此欄位為必填".into()),
            (1, "product_id".into(), "不是有效的 id".into()),
            (1, "qty".into(), "必須是數字".into()),
            (2, "qty".into(), "數量必須大於 0".into()),
            (3, "qty".into(), "最多 4 位小數".into()),
        ]
    );

    // 刪除的商品、找不到的批號
    let (status, _) = call(
        &pool,
        Method::DELETE,
        &format!("{}?version=1", record_uri("product", m.other)),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = preview(
        &pool,
        &token,
        json!([
            { "product_id": m.other, "qty": "1" },
            { "product_id": m.goods, "qty": "1", "lot_no": "NOPE" },
            { "product_id": Id::now_v7(), "qty": "1" },
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"],
        json!([
            { "detail_key": null, "index": 0, "field_key": "product_id", "message": "商品不存在" },
            { "detail_key": null, "index": 1, "field_key": "lot_no", "message": "找不到此批號" },
            { "detail_key": null, "index": 2, "field_key": "product_id", "message": "商品不存在" },
        ])
    );

    // 沒有登入
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri(PREVIEW)
        .header(header::USER_AGENT, "HammerERP-test")
        .header("X-Client-Version", "0.1.0")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "lines": [] }).to_string()))
        .unwrap();
    let res = router(state(&pool)).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
