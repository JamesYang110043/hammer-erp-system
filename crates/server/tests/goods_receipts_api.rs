//! 進貨單的整合測試（#140，docs/architecture.md §8.8「採購單轉進貨單」「進貨過帳與作廢」、§9.1、§9.3）：
//! 存檔時的檢查與帶入、外幣成本、建立批號、回寫採購單的已進貨數量與狀態、作廢的限制與回復。
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
    supplier: Id,
    goods: Id,
    other: Id,
    service: Id,
}

async fn master_data(pool: &PgPool, token: &str) -> Master {
    let supplier = create(
        pool,
        token,
        "supplier",
        json!({ "data": { "code": "S001", "name": "美國廠商", "default_currency_code": "USD" } }),
    )
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
        supplier,
        goods,
        other,
        service,
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

fn lines<'a>(record: &'a Value, detail: &str) -> &'a Vec<Value> {
    record["lines"][detail].as_array().unwrap()
}

/// 已確認的美元採購單：A001 10 個 @ 12.345、A002 5 個 @ 2。
async fn confirmed_order(pool: &PgPool, token: &str, m: &Master) -> Value {
    let po = create(
        pool,
        token,
        "purchase_order",
        json!({
            "data": { "supplier_id": m.supplier, "order_date": "2026-11-01",
                      "exchange_rate": "30" },
            "lines": { "purchase_order_line": [
                { "data": { "product_id": m.goods, "qty": "10", "unit_price": "12.345" } },
                { "data": { "product_id": m.other, "qty": "5", "unit_price": "2" } },
            ] },
        }),
    )
    .await;
    let (status, body) = action(pool, token, "purchase_order", po.id, "confirm", po.version).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn get(pool: &PgPool, token: &str, entity: &str, id: &Value) -> Value {
    let id: Id = serde_json::from_value(id.clone()).unwrap();
    let (status, body) = call(pool, Method::GET, &record_uri(entity, id), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn create_receipt(pool: &PgPool, token: &str, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        Method::POST,
        &records_uri("goods_receipt"),
        token,
        Some(body),
    )
    .await
}

async fn post_receipt(pool: &PgPool, token: &str, receipt: &Value) -> (StatusCode, Value) {
    let id: Id = serde_json::from_value(receipt["id"].clone()).unwrap();
    let version = i32::try_from(receipt["version"].as_i64().unwrap()).unwrap();
    action(pool, token, "goods_receipt", id, "post", version).await
}

async fn void_receipt(pool: &PgPool, token: &str, receipt: &Value) -> (StatusCode, Value) {
    let id: Id = serde_json::from_value(receipt["id"].clone()).unwrap();
    let version = i32::try_from(receipt["version"].as_i64().unwrap()).unwrap();
    action(pool, token, "goods_receipt", id, "void", version).await
}

async fn lots(pool: &PgPool, token: &str, product: Id) -> Vec<Value> {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/inventory/products/{product}/lots?include_empty=true"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

async fn transactions(pool: &PgPool, token: &str, product: Id) -> Vec<Value> {
    let (status, body) = call(
        pool,
        Method::GET,
        &format!("/api/v1/inventory/transactions?product_id={product}"),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

/// 外幣進貨分兩次過帳：成本換算台幣、建立批號、採購單依序變為部分進貨、結案。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn foreign_receipts_create_lots_and_close_order(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    exchange_rate(&pool, &token, "USD", "2026-11-15", "32.15").await;
    let po = confirmed_order(&pool, &token, &m).await;
    let po_lines = lines(&po, "purchase_order_line");
    let (l1, l2) = (po_lines[0]["id"].clone(), po_lines[1]["id"].clone());

    // 只帶採購單：廠商、幣別、稅別帶入採購單的值，匯率依進貨日期（不沿用採購單的 30）
    let (status, gr) = create_receipt(
        &pool,
        &token,
        json!({
            "data": { "purchase_order_id": po["id"], "receipt_date": "2026-11-17" },
            "lines": { "goods_receipt_line": [
                { "data": { "purchase_order_line_id": l1, "product_id": m.goods, "qty": "4",
                            "unit_price": "12.345", "lot_no": " L-01 " } },
                { "data": { "purchase_order_line_id": l2, "product_id": m.other, "qty": "5",
                            "unit_price": "2" } },
                // 沒有採購明細的運費
                { "data": { "product_id": m.service, "qty": "1", "unit_price": "7.5" } },
            ] },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{gr}");
    assert_eq!(gr["data"]["supplier_id"], json!(m.supplier));
    assert_eq!(gr["data"]["currency_code"], "USD");
    assert_eq!(gr["data"]["tax_type"], "zero_rated");
    assert_eq!(gr["data"]["exchange_rate"], "32.150000");
    // 金額：49.38 + 10.00 + 7.50
    assert_eq!(gr["data"]["subtotal"], "66.8800");
    assert_eq!(gr["data"]["total_twd"], "2150.19");
    assert_eq!(
        lines(&gr, "goods_receipt_line")[0]["data"]["lot_no"],
        "L-01"
    );

    let (status, posted) = post_receipt(&pool, &token, &gr).await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    assert_eq!(posted["data"]["status"], "posted");
    let gr_lines = lines(&posted, "goods_receipt_line");
    // 台幣成本：round(12.345 × 32.15, 4)
    assert_eq!(gr_lines[0]["data"]["unit_cost_twd"], "396.8918");
    assert_eq!(gr_lines[1]["data"]["unit_cost_twd"], "64.3000");
    assert_eq!(gr_lines[2]["data"]["unit_cost_twd"], "241.1250");
    // 批號留空時自動產生；服務沒有批號
    let record_no = posted["record_no"].as_str().unwrap();
    assert_eq!(gr_lines[1]["data"]["lot_no"], format!("{record_no}-02"));
    assert!(gr_lines[2]["data"].get("lot_no").is_none());

    let a_lots = lots(&pool, &token, m.goods).await;
    assert_eq!(a_lots.len(), 1);
    assert_eq!(a_lots[0]["lot_no"], "L-01");
    assert_eq!(a_lots[0]["qty_in"], "4.0000");
    assert_eq!(a_lots[0]["unit_cost_twd"], "396.8918");
    assert_eq!(a_lots[0]["received_date"], "2026-11-17");
    assert_eq!(a_lots[0]["source_record_no"], record_no);
    assert!(lots(&pool, &token, m.service).await.is_empty());
    let txns = transactions(&pool, &token, m.goods).await;
    assert_eq!(txns.len(), 1);
    assert_eq!(txns[0]["txn_type"], "receipt");
    assert_eq!(txns[0]["qty_change"], "4.0000");
    assert_eq!(txns[0]["ref_record_no"], record_no);

    // 採購單：部分進貨，已進貨數量回寫
    let po_now = get(&pool, &token, "purchase_order", &po["id"]).await;
    assert_eq!(po_now["data"]["status"], "partially_received");
    let po_now_lines = lines(&po_now, "purchase_order_line");
    assert_eq!(po_now_lines[0]["data"]["received_qty"], "4.0000");
    assert_eq!(po_now_lines[1]["data"]["received_qty"], "5.0000");
    assert_eq!(po_now["version"], po["version"].as_i64().unwrap() + 1);
    let po_id: Id = serde_json::from_value(po["id"].clone()).unwrap();
    let actions: Vec<(String, Value)> = sqlx::query_as(
        "SELECT action, after_data FROM audit_logs WHERE record_id = $1 ORDER BY occurred_at, id",
    )
    .bind(po_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actions.last().unwrap().0, "update");
    assert_eq!(actions.last().unwrap().1["status"], "partially_received");

    // 超收被拒絕：剩 6，輸入 7
    let receipt_body = |qty: &str| {
        json!({
            "data": { "purchase_order_id": po["id"], "receipt_date": "2026-11-18" },
            "lines": { "goods_receipt_line": [
                { "data": { "purchase_order_line_id": l1, "product_id": m.goods, "qty": qty,
                            "unit_price": "12" } },
            ] },
        })
    };
    let (status, body) = create_receipt(&pool, &token, receipt_body("7")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["lines"][0]["message"],
        "超過未進貨數量 6.0000"
    );

    // 兩張草稿各自輸入全部的未進貨數量：先過帳的成功，後過帳的失敗
    let (_, first) = create_receipt(&pool, &token, receipt_body("6")).await;
    let (status, second) = create_receipt(&pool, &token, receipt_body("6")).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let (status, body) = post_receipt(&pool, &token, &first).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let po_now = get(&pool, &token, "purchase_order", &po["id"]).await;
    // 全部進貨：自動結案（不是手動，closed_at 為空）
    assert_eq!(po_now["data"]["status"], "closed");
    assert!(po_now["document"]["closed_at"].is_null());

    let (status, body) = post_receipt(&pool, &token, &second).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "purchase_order_id", "message": "採購單已結案" }])
    );
    assert_eq!(
        body["details"]["lines"][0]["message"],
        "超過未進貨數量 0.0000"
    );
    let second_now = get(&pool, &token, "goods_receipt", &second["id"]).await;
    assert_eq!(second_now["data"]["status"], "draft");
}

/// 作廢：刪除批號、扣回採購單的已進貨數量並重新計算狀態；批號已被使用時拒絕並完全回復。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn void_reverses_receipt(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let po = confirmed_order(&pool, &token, &m).await;
    let l1 = lines(&po, "purchase_order_line")[0]["id"].clone();

    let (_, gr) = create_receipt(
        &pool,
        &token,
        json!({
            "data": { "purchase_order_id": po["id"], "receipt_date": "2026-11-17",
                      "exchange_rate": "32" },
            "lines": { "goods_receipt_line": [
                { "data": { "purchase_order_line_id": l1, "product_id": m.goods, "qty": "10",
                            "unit_price": "1", "lot_no": "L-01" } },
            ] },
        }),
    )
    .await;
    let (status, posted) = post_receipt(&pool, &token, &gr).await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    // A001 全部進貨，A002 還沒 → 部分進貨
    let po_now = get(&pool, &token, "purchase_order", &po["id"]).await;
    assert_eq!(po_now["data"]["status"], "partially_received");

    // 批號被盤點調整單扣用 → 作廢被拒絕，訊息指出批號
    let adjustment = create(
        &pool,
        &token,
        "stock_adjustment",
        json!({
            "data": { "adjustment_date": "2026-11-18" },
            "lines": { "stock_adjustment_line": [
                { "data": { "product_id": m.goods, "line_type": "loss", "lot_no": "L-01",
                            "qty": "1" } },
            ] },
        }),
    )
    .await;
    let (status, adj) = action(
        &pool,
        &token,
        "stock_adjustment",
        adjustment.id,
        "post",
        adjustment.version,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{adj}");
    let (status, body) = void_receipt(&pool, &token, &posted).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "LOT_IN_USE");
    assert!(body["message"].as_str().unwrap().contains("L-01"), "{body}");
    assert_eq!(body["details"]["lots"][0]["lot_no"], "L-01");
    // 完全回復
    let po_after = get(&pool, &token, "purchase_order", &po["id"]).await;
    assert_eq!(po_after["version"], po_now["version"]);
    assert_eq!(
        lines(&po_after, "purchase_order_line")[0]["data"]["received_qty"],
        "10.0000"
    );
    assert_eq!(lots(&pool, &token, m.goods).await.len(), 1);

    // 作廢盤點調整單後可以作廢進貨單
    let (status, _) = action(&pool, &token, "stock_adjustment", adjustment.id, "void", 2).await;
    assert_eq!(status, StatusCode::OK);
    let (status, voided) = void_receipt(&pool, &token, &posted).await;
    assert_eq!(status, StatusCode::OK, "{voided}");
    assert_eq!(voided["data"]["status"], "void");
    assert!(lots(&pool, &token, m.goods).await.is_empty());
    let txns = transactions(&pool, &token, m.goods).await;
    let receipt_txns: Vec<&Value> = txns
        .iter()
        .filter(|t| t["ref_doc_type"] == "goods_receipt")
        .collect();
    assert_eq!(receipt_txns.len(), 2);
    assert_eq!(receipt_txns[0]["txn_type"], "void_reversal");
    assert_eq!(receipt_txns[0]["qty_change"], "-10.0000");
    let po_after = get(&pool, &token, "purchase_order", &po["id"]).await;
    assert_eq!(po_after["data"]["status"], "confirmed");
    assert_eq!(
        lines(&po_after, "purchase_order_line")[0]["data"]["received_qty"],
        "0.0000"
    );
}

/// 手動結案的採購單：作廢進貨後維持結案。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn void_keeps_manually_closed_order(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;
    let po = confirmed_order(&pool, &token, &m).await;
    let l1 = lines(&po, "purchase_order_line")[0]["id"].clone();
    let (_, gr) = create_receipt(
        &pool,
        &token,
        json!({
            "data": { "purchase_order_id": po["id"], "receipt_date": "2026-11-17",
                      "exchange_rate": "32" },
            "lines": { "goods_receipt_line": [
                { "data": { "purchase_order_line_id": l1, "product_id": m.goods, "qty": "3",
                            "unit_price": "1" } },
            ] },
        }),
    )
    .await;
    let (_, posted) = post_receipt(&pool, &token, &gr).await;
    let po_now = get(&pool, &token, "purchase_order", &po["id"]).await;
    let po_id: Id = serde_json::from_value(po["id"].clone()).unwrap();
    let version = i32::try_from(po_now["version"].as_i64().unwrap()).unwrap();
    let (status, _) = action(&pool, &token, "purchase_order", po_id, "close", version).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = void_receipt(&pool, &token, &posted).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let po_after = get(&pool, &token, "purchase_order", &po["id"]).await;
    assert_eq!(po_after["data"]["status"], "closed");
    assert!(po_after["document"]["closed_at"].is_string());
    assert_eq!(
        lines(&po_after, "purchase_order_line")[0]["data"]["received_qty"],
        "0.0000"
    );
}

/// 有已過帳的進貨退回時不可作廢。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn void_blocked_by_posted_return(pool: PgPool) {
    let (user_id, token) = create_user_with_session(&pool, &TestUser::new("editor", role::EDITOR))
        .await
        .unwrap();
    let token = token.into_string();
    let m = master_data(&pool, &token).await;
    let (_, gr) = create_receipt(
        &pool,
        &token,
        json!({
            "data": { "supplier_id": m.supplier, "receipt_date": "2026-11-17",
                      "exchange_rate": "32" },
            "lines": { "goods_receipt_line": [
                { "data": { "product_id": m.goods, "qty": "3", "unit_price": "1" } },
            ] },
        }),
    )
    .await;
    let (status, posted) = post_receipt(&pool, &token, &gr).await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    let gr_line = lines(&posted, "goods_receipt_line")[0]["id"].clone();
    let ret = create(
        &pool,
        &token,
        "purchase_return",
        json!({
            "data": { "goods_receipt_id": posted["id"], "return_date": "2026-11-18" },
            "lines": { "purchase_return_line": [
                { "data": { "goods_receipt_line_id": gr_line, "qty": "1" } },
            ] },
        }),
    )
    .await;
    // 進貨退回的過帳在 #141，這裡直接設定狀態
    sqlx::query(
        "UPDATE purchase_returns SET status = 'posted', posted_at = now(), posted_by = $2 \
         WHERE id = $1",
    )
    .bind(ret.id)
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = void_receipt(&pool, &token, &posted).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "VALIDATION");
    let record_no = ret.record_no.unwrap();
    assert_eq!(
        body["message"],
        format!("此進貨單已有進貨退回 {record_no}，請先作廢退回單")
    );
    assert_eq!(
        body["details"],
        json!({ "fields": [], "documents": [
            { "entity_key": "purchase_return", "record_id": ret.id, "record_no": record_no },
        ] })
    );
}

/// 存檔與過帳時的檢查：批號、服務、採購單的狀態與廠商、匯率、至少一列明細。
#[sqlx::test(migrator = "hammer_core::db::MIGRATOR")]
async fn receipt_validation(pool: PgPool) {
    let token = user(&pool, "editor", role::EDITOR).await;
    let m = master_data(&pool, &token).await;

    // 既有批號
    let (_, gr) = create_receipt(
        &pool,
        &token,
        json!({
            "data": { "supplier_id": m.supplier, "receipt_date": "2026-11-17",
                      "exchange_rate": "32" },
            "lines": { "goods_receipt_line": [
                { "data": { "product_id": m.goods, "qty": "1", "unit_price": "1",
                            "lot_no": "L-01" } },
            ] },
        }),
    )
    .await;
    let (status, _) = post_receipt(&pool, &token, &gr).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = create_receipt(
        &pool,
        &token,
        json!({
            "data": { "supplier_id": m.supplier, "receipt_date": "2026-11-17" },
            "lines": { "goods_receipt_line": [
                { "data": { "product_id": m.goods, "qty": "1", "unit_price": "1",
                            "lot_no": "l-01" } },
                { "data": { "product_id": m.other, "qty": "1", "unit_price": "1",
                            "lot_no": "X" } },
                { "data": { "product_id": m.other, "qty": "1", "unit_price": "1",
                            "lot_no": "x" } },
                { "data": { "product_id": m.service, "qty": "1", "unit_price": "1",
                            "lot_no": "S" } },
                { "data": { "product_id": m.goods, "qty": "0", "unit_price": "1" } },
            ] },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let mut got: Vec<(i64, String)> = body["details"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["index"].as_i64().unwrap(),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            (0, "批號已存在".to_owned()),
            (2, "批號重複".to_owned()),
            (3, "服務類商品沒有批號".to_owned()),
            (4, "數量必須大於 0".to_owned()),
        ]
    );

    // 草稿的採購單、廠商不同
    let draft_po = create(
        &pool,
        &token,
        "purchase_order",
        json!({ "data": { "supplier_id": m.supplier, "order_date": "2026-11-01" } }),
    )
    .await;
    let other_supplier = create(
        &pool,
        &token,
        "supplier",
        json!({ "data": { "code": "S002", "name": "其他廠商" } }),
    )
    .await
    .id;
    let (status, body) = create_receipt(
        &pool,
        &token,
        json!({ "data": { "purchase_order_id": draft_po.id, "supplier_id": other_supplier,
                          "currency_code": "TWD", "receipt_date": "2026-11-17" } }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([
            { "field_key": "purchase_order_id", "message": "採購單尚未確認" },
            { "field_key": "supplier_id", "message": "廠商與採購單不同" },
            { "field_key": "currency_code", "message": "幣別與採購單不同" },
        ])
    );

    // 過帳：沒有匯率、沒有明細
    let (status, empty) = create_receipt(
        &pool,
        &token,
        json!({ "data": { "supplier_id": m.supplier, "receipt_date": "2026-10-01" } }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{empty}");
    assert!(empty["data"].get("exchange_rate").is_none());
    let (status, body) = post_receipt(&pool, &token, &empty).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["details"]["fields"],
        json!([{ "field_key": "exchange_rate", "message": "找不到匯率，請輸入匯率" }])
    );
    assert_eq!(body["details"]["lines"][0]["message"], "至少要有一筆明細");

    // 已過帳的進貨單不可修改
    let (status, body) = call(
        &pool,
        Method::PATCH,
        &record_uri(
            "goods_receipt",
            serde_json::from_value(gr["id"].clone()).unwrap(),
        ),
        &token,
        Some(json!({ "version": 2, "data": { "notes": "x" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "INVALID_STATUS");
}
